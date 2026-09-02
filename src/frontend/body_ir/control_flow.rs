//! Lowering for conditional and looping control flow, and the iteration protocol behind `for`.

use super::args::*;
use super::primitives::*;
use super::reads::*;
use super::refusals::*;
use super::*;

/// Where one range-shaped `for` loop takes its bounds, step, and inclusivity from.
///
/// The two variants are the two ways the surface can spell a range in iterable position, and they differ only in
/// where those facts live: written into the header, or carried by an already-built range value. Both drive the
/// same normalized counting loop -- see [`BodyBuilder::lower_range_counting_loop`].
enum RangeLoopSource<'ast> {
    /// An inline `start..end` / `start..=end` loop header, still holding its un-lowered bound expressions.
    Header {
        start: &'ast ast::Spanned<ast::Expr>,
        end: &'ast ast::Spanned<ast::Expr>,
        inclusive: bool,
    },
    /// A materialized [`bir::AggregateKind::Range`] value, read back through its own declared fields.
    Value(bir::Place),
}

/// Read a counting loop's index local.
///
/// The index is a compiler-owned `int` temporary that the loop itself both writes and re-reads several times per
/// iteration, so its reads are always plain [`bir::OwnershipFact::Copy`] and never a last use. Going through
/// [`BodyBuilder::ownership_fact_for_place`] would consult a read countdown that was never seeded for a temporary.
fn index_read(idx_local: bir::LocalId) -> bir::Operand {
    bir::Operand::place(bir::Place::from_local(idx_local), bir::OwnershipFact::Copy, false)
}

/// Retain a local range-layout fact only when every control-flow successor retains it.
pub(super) fn intersect_range_layouts(
    states: Vec<std::collections::HashSet<bir::LocalId>>,
) -> std::collections::HashSet<bir::LocalId> {
    let mut states = states.into_iter();
    let Some(mut common) = states.next() else {
        return std::collections::HashSet::new();
    };
    for state in states {
        common.retain(|local| state.contains(local));
    }
    common
}

/// Return the immediate inline range parts through redundant source parentheses.
fn inline_range_parts(
    expr: &ast::Spanned<ast::Expr>,
) -> Option<(&ast::Spanned<ast::Expr>, &ast::Spanned<ast::Expr>, bool)> {
    match &expr.node {
        ast::Expr::Range { start, end, inclusive } => Some((start, end, *inclusive)),
        ast::Expr::Paren(inner) => inline_range_parts(inner),
        _ => None,
    }
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower `if`/`elif`/`else` into a [`bir::StatementKind::If`] chain. `elif` branches are folded into nested
    /// `else { if ... }` wrappers from the last branch inward (see the inline comment above the fold loop), and an
    /// `if let` pattern condition — not yet modeled by v0 — lowers to an explicit unsupported placeholder instead of
    /// the real branch.
    pub(super) fn lower_if(
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

        let range_layouts_before = self.materialized_range_locals.clone();
        let then_block = self.lower_branch_block(&if_stmt.then_body, scope, span);
        let mut range_layouts_after_branches = vec![self.materialized_range_locals.clone()];

        self.materialized_range_locals = range_layouts_before.clone();
        let mut else_block = if_stmt
            .else_body
            .as_ref()
            .map(|else_body| self.lower_branch_block(else_body, scope, span));
        range_layouts_after_branches.push(self.materialized_range_locals.clone());

        // Fold `elif` branches into nested `else { if ... }` wrappers, innermost (last elif) first, so the earlier
        // conditions end up evaluated first at the top of the chain once wrapped by the outer `if` pushed below.
        for (elif_cond, elif_body) in if_stmt.elif_branches.iter().rev() {
            self.materialized_range_locals = range_layouts_before.clone();
            let mut wrapper = Vec::new();
            let cond_operand = self.lower_expr_to_operand(elif_cond, scope, &mut wrapper);
            let then_block = self.lower_branch_block(elif_body, scope, span);
            wrapper.push(bir::Statement {
                kind: bir::StatementKind::If {
                    cond: cond_operand,
                    then_block,
                    else_block,
                },
                span,
            });
            else_block = Some(bir::Block { scope, stmts: wrapper });
            range_layouts_after_branches.push(self.materialized_range_locals.clone());
        }

        self.materialized_range_locals = intersect_range_layouts(range_layouts_after_branches);

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
    pub(super) fn lower_branch_block(
        &mut self,
        body: &[ast::Spanned<ast::Statement>],
        parent_scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::Block {
        // Body IR retains every branch local for the backend to lower, but its name lookup must return to the
        // enclosing lexical environment afterwards. Without this snapshot a `let x` in one branch becomes the
        // binding read after the `if`, even though source resolution scopes it to the branch alone.
        let enclosing_bindings = self.bindings.clone();
        let branch_scope = self.new_scope(Some(parent_scope), span);
        let mut stmts = Vec::new();
        self.lower_block_into(body, branch_scope, &mut stmts);
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
    /// `src/frontend/typechecker/check_expr/control_flow.rs` discards any branch value and always returns
    /// `ResolvedType::Unit`) -- unlike a `loop` expression, an `if` expression cannot yet produce a value from its
    /// branches, so its Body IR operand is always the `Unit` constant rather than a place read.
    pub(super) fn lower_if_expr(
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
            intersect_range_layouts(vec![then_range_layouts, self.materialized_range_locals.clone()]);
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
    /// `loop_scope` is the caller's to create rather than this helper's, because the `loop:` expression must
    /// declare its result temporary in that scope before any body statement is lowered.
    ///
    /// The two `for` paths deliberately do not route through here. [`Self::lower_range_counting_loop`] drops its
    /// scope mid-body, before the index advance it appends afterwards, and [`Self::lower_general_iteration`] leaves
    /// the drops to its caller's body closure; both would need this helper's fixed "body, then drops" order relaxed
    /// into a per-caller choice, which would buy nothing but an extra parameter.
    fn push_loop(
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
        let mut body_stmts = Vec::new();
        lower_body(self, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);
        self.loop_break_targets.pop();
        self.materialized_range_locals =
            intersect_range_layouts(vec![range_layouts_before, self.materialized_range_locals.clone()]);
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
    fn lower_if_let(
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
        let scrutinee_place = self.lower_expr_to_place(value, scope, out);
        let scrutinee = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

        let layouts_before = self.materialized_range_locals.clone();
        let matched_arm =
            self.lower_statement_pattern_arm(pattern, &scrutinee_ty, &scrutinee_place, scope, then_body, span);
        let layouts_after_match = self.materialized_range_locals.clone();

        // The unmatched path runs none of the body, so it leaves the entry facts untouched.
        self.materialized_range_locals = intersect_range_layouts(vec![layouts_before, layouts_after_match]);

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
    fn lower_while_let(
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
            let scrutinee_place = builder.lower_expr_to_place(value, loop_scope, body_stmts);
            let scrutinee = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

            // Exactly one arm runs per iteration, so a fact the matched arm established must not survive into code
            // that could instead have reached the exhausted arm. `push_loop` intersects at the loop boundary, which
            // covers today's shape because the match is the body's only statement -- but relying on that would make
            // this correct by accident, and wrong the moment a statement is appended after it.
            let layouts_before_arms = builder.materialized_range_locals.clone();
            let matched_arm =
                builder.lower_statement_pattern_arm(pattern, &scrutinee_ty, &scrutinee_place, loop_scope, body, span);
            builder.materialized_range_locals =
                intersect_range_layouts(vec![layouts_before_arms, builder.materialized_range_locals.clone()]);
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
    /// `src/frontend/typechecker/check_expr/control_flow.rs`), so -- unlike an `if` expression, which is always
    /// `Unit` -- a `loop` expression's produced value genuinely comes from its branches and needs this
    /// merge-into-one-place treatment; see [`Self::lower_break`] for the other half of the mechanism.
    pub(super) fn lower_loop_expr(
        &mut self,
        loop_expr: &ast::LoopExpr,
        scope: bir::ScopeId,
        span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let loop_scope = self.new_scope(Some(scope), hir_span_value);
        let result_local = self.new_temp(ty.clone(), loop_scope, hir_span_value);

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
    /// unifies the break types (both in `src/frontend/typechecker/check_stmt.rs`). Passing `None` here is that same
    /// rule read off the typechecker rather than a second one invented in lowering: a well-typed program has no
    /// value-carrying `break` to route anywhere, and a hand-built AST that has one keeps its value on the `Break`
    /// statement per [`bir::StatementKind::Break`]'s documented default instead of being silently merged into some
    /// enclosing loop's result.
    ///
    /// Scoping mirrors [`Self::lower_while`], which the typechecker treats identically (`check_while_stmt` opens
    /// the same `ScopeKind::Block` and pushes the same statement loop context). `continue` needs nothing here at
    /// all: it lowers to [`bir::StatementKind::Continue`] in [`Self::lower_stmt_into`] and means the innermost
    /// enclosing loop wherever it appears.
    pub(super) fn lower_loop_stmt(
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
    pub(super) fn lower_while(
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
    /// `src/frontend/typechecker/check_stmt.rs` (#1125). A plain `for x in ...` binds the produced item directly;
    /// every other shape writes it into a per-iteration temporary that [`Self::bind_for_pattern`] then projects one
    /// real named binding out of per bound name. Any shape outside that subset -- which the typechecker already
    /// rejects with its own diagnostic before lowering ever runs -- lowers to `Unsupported` naming the offending
    /// shape, checked up front so a refusal never leaves half-emitted bindings behind (the same
    /// "check before partially lowering" precedent as [`Self::lower_binary`] and [`Self::lower_match`]). The same
    /// up-front check also refuses a tuple pattern whose produced item is not a tuple of matching arity, so
    /// lowering can never invent `.0`/`.1` projections into a value that has no such fields -- see
    /// [`unsupported_for_pattern`].
    pub(super) fn lower_for(
        &mut self,
        for_stmt: &ast::ForStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let iter_ty = self.resolve_ty(for_stmt.iter.span);
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
        let range = match self.range_loop_source(&for_stmt.iter, &iter_ty) {
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
                intersect_range_layouts(vec![range_layouts_before, self.materialized_range_locals.clone()]);
            return;
        };
        self.lower_range_counting_loop(for_stmt, &item_ty, &range, scope, span, out);
        self.bindings = enclosing_bindings;
        self.materialized_range_locals =
            intersect_range_layouts(vec![range_layouts_before, self.materialized_range_locals.clone()]);
    }

    /// The item type a `for` loop's pattern binds, falling back to a range value's own element type.
    ///
    /// Normally this is just the typechecker's recorded type for the pattern span. The generic `Range[T]` spelling
    /// carries its item type, so it supplies the recovery fallback only when the checker has no fact. That type
    /// fallback never grants aggregate-layout authority; [`Self::range_loop_source`] separately requires local
    /// materialization provenance before field projection.
    fn for_item_type(&self, pattern: &ast::Spanned<ast::Pattern>, iter_ty: &IncanType) -> IncanType {
        let checked = self.resolve_ty(pattern.span);
        if !matches!(checked, IncanType::Unknown) {
            return checked;
        }
        range_value_element_type(iter_ty).cloned().unwrap_or(checked)
    }

    /// Classify a `for` header's iterable as a range the counting loop can drive, lowering it to a place when it
    /// is a range *value*, or `None` when the loop belongs on the general-iteration path.
    ///
    /// An inline `start..end` header keeps its AST sub-expressions, because its bounds are still lowered directly
    /// (and its `end` deliberately re-lowered per iteration -- see [`Self::lower_range_counting_loop`]). A bound
    /// range needs an independently proven local [`bir::AggregateKind::Range`] producer. A `Range[T]` type spelling
    /// alone cannot prove the aggregate layout: parameters, call results, imported values, and user declarations
    /// may carry it without `start`/`end`/`step`/`inclusive` fields. Those cases refuse visibly rather than inventing
    /// a private range ABI. The `range()` builtin resolves to a different type and keeps its existing iteration path.
    fn range_loop_source<'ast>(
        &self,
        iter_expr: &'ast ast::Spanned<ast::Expr>,
        iter_ty: &IncanType,
    ) -> Result<Option<RangeLoopSource<'ast>>, String> {
        if let Some((start, end, inclusive)) = inline_range_parts(iter_expr) {
            return Ok(Some(RangeLoopSource::Header { start, end, inclusive }));
        }
        if range_value_element_type(iter_ty).is_none() {
            return Ok(None);
        }
        let Some(place) = self.materialized_range_place(iter_expr) else {
            return Err("range value without a source-local Body IR range aggregate".to_string());
        };
        Ok(Some(RangeLoopSource::Value(place)))
    }

    /// Whether `expr` is a source-local value whose current Body-IR representation has the declared range layout.
    /// This follows only direct range literals, parentheses, and copies of another proven local; calls, imports,
    /// parameters, fields, and arbitrary type spellings remain unproven.
    pub(super) fn expr_has_materialized_range_layout(&self, expr: &ast::Spanned<ast::Expr>) -> bool {
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

    /// Return the plain local place for a range value whose layout provenance is still live.
    fn materialized_range_place(&self, expr: &ast::Spanned<ast::Expr>) -> Option<bir::Place> {
        match &expr.node {
            ast::Expr::Paren(inner) => self.materialized_range_place(inner),
            ast::Expr::Ident(name) => self
                .bindings
                .get(name)
                .copied()
                .filter(|local| self.materialized_range_locals.contains(local))
                .map(bir::Place::from_local),
            _ => None,
        }
    }

    /// Read one declared field off a materialized [`bir::AggregateKind::Range`] value.
    ///
    /// Every range field is a scalar read through a non-empty projection, so this always resolves to
    /// [`bir::OwnershipFact::Copy`] and never consumes the range local's last use -- a loop may read the same
    /// field on every iteration without the range appearing to be moved out from under itself.
    fn read_range_field(&mut self, range: &bir::Place, field: &str, field_ty: &IncanType) -> bir::Operand {
        let mut place = range.clone();
        place.projection.push(bir::PlaceElem::synthetic_field(field));
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
    fn range_value_stop_condition(
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

    /// Lower one range-shaped `for` loop into the normalized counting `Loop` both range spellings share: seed an
    /// index from the range's start, break once it reaches the end, bind the loop pattern from the index, run the
    /// body, then advance the index by the range's step.
    ///
    /// Where the two spellings get those three pieces from is the only difference, and each is taken from
    /// `source`: an inline header lowers its own AST sub-expressions and knows its step and inclusivity
    /// statically, while a range value reads them back off the place it was materialized into. The `end` bound is
    /// evaluated *inside* the loop body in both cases, preserving the inline form's established re-evaluation
    /// timing rather than hoisting it.
    fn lower_range_counting_loop(
        &mut self,
        for_stmt: &ast::ForStmt,
        item_ty: &IncanType,
        source: &RangeLoopSource<'_>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let start_operand = match source {
            RangeLoopSource::Header { start, .. } => self.lower_expr_to_operand(start, scope, out),
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

        let loop_scope = self.new_scope(Some(scope), span);
        // `for` never produces a value from `break` (same reasoning as `while` -- see `Self::lower_while`).
        self.loop_break_targets.push(None);
        let mut body_stmts = Vec::new();

        let cond = match source {
            RangeLoopSource::Header { end, inclusive, .. } => {
                let end_operand = self.lower_expr_to_operand(end, loop_scope, &mut body_stmts);
                let cmp_op = if *inclusive { bir::BinOp::Gt } else { bir::BinOp::Ge };
                self.push_assign_temp(
                    bir::Rvalue::BinaryOp(cmp_op, index_read(idx_local), end_operand),
                    IncanType::Primitive(IncanPrimitiveType::Bool),
                    loop_scope,
                    span,
                    &mut body_stmts,
                )
            }
            RangeLoopSource::Value(range) => {
                self.range_value_stop_condition(range, idx_local, loop_scope, span, &mut body_stmts)
            }
        };
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

        // `for _ in start..end` binds nothing and the range's own index already drives the loop, so it needs no
        // per-iteration item local at all -- unlike the general path, where `IterNext` must still write the polled
        // item somewhere for the poll itself to happen.
        if !matches!(for_stmt.pattern.node, ast::Pattern::Wildcard) {
            let item_local = self.declare_for_item_local(
                &for_stmt.pattern,
                item_ty,
                loop_scope,
                hir_span(for_stmt.pattern.span),
                &|name| count_reads_in_stmts(name, &for_stmt.body),
            );
            body_stmts.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(item_local),
                    rvalue: bir::Rvalue::Use(index_read(idx_local)),
                },
                span,
            });
            self.bind_for_pattern(
                &for_stmt.pattern,
                item_ty,
                item_local,
                loop_scope,
                &|name| count_reads_in_stmts(name, &for_stmt.body),
                &mut body_stmts,
            );
        }

        self.lower_block_into(&for_stmt.body, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);

        let step = match source {
            RangeLoopSource::Header { .. } => bir::Operand::Constant(bir::Constant::Int(RANGE_UNIT_STEP)),
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
        self.loop_break_targets.pop();

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

    /// Declare the local each produced item of a `for` loop is written into.
    ///
    /// A plain `for x in ...` binds the item directly: the item local *is* `x`'s local, so the produced value is
    /// never copied and the loop shape #1103/#1101 established is preserved byte-for-byte. Every other supported
    /// pattern shape has no single name to write into, so the item goes into a temporary that
    /// [`Self::bind_for_pattern`] projects the real bindings out of -- the same "materialize once, then bind each
    /// element off a projection" shape [`Self::lower_tuple_unpack`] already uses for `a, b = value`.
    pub(super) fn declare_for_item_local(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        item_ty: &IncanType,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        reads: &dyn Fn(&str) -> usize,
    ) -> bir::LocalId {
        match &pattern.node {
            ast::Pattern::Binding(name) => {
                let total_reads = reads(name);
                self.declare_new_local_with_reads(name.clone(), item_ty.clone(), loop_scope, span, total_reads)
            }
            _ => self.new_temp(item_ty.clone(), loop_scope, span),
        }
    }

    /// Emit the binding statements a `for` loop's pattern needs against the item local, immediately after the
    /// per-iteration `IterNext` (or, on the range path, after the index copy) has written it.
    ///
    /// A bare [`ast::Pattern::Binding`] emits nothing: [`Self::declare_for_item_local`] already declared the item
    /// local *as* that binding, so there is nothing left to project. Every other shape delegates to
    /// [`Self::bind_for_pattern_fields`], which means every binding that walk reaches is nested under at least one
    /// tuple field and therefore always reads through a projection.
    pub(super) fn bind_for_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        item_ty: &IncanType,
        item_local: bir::LocalId,
        loop_scope: bir::ScopeId,
        reads: &dyn Fn(&str) -> usize,
        out: &mut Vec<bir::Statement>,
    ) {
        if matches!(pattern.node, ast::Pattern::Binding(_)) {
            return;
        }
        let item_place = bir::Place::from_local(item_local);
        self.bind_for_pattern_fields(pattern, item_ty, &item_place, loop_scope, reads, out);
    }

    /// Recursively bind one `for`-pattern node against `place`, the (already projected) part of the produced item
    /// it corresponds to, emitting one `Assign` per bound name in source order.
    ///
    /// Iteration binding is *irrefutable*: unlike [`Self::lower_match_pattern`], which builds a [`bir::Pattern`]
    /// for match-arm dispatch, there is nothing here to test or branch on, so this walk emits plain assignments and
    /// deliberately does not reuse that machinery (#1125 names conflating the two as a non-goal). What it does
    /// share is that walk's projection convention -- the zero-based tuple-element index spelled as a
    /// [`bir::PlaceElem::Field`], matching [`Self::lower_tuple_unpack`]'s `.0`/`.1` spelling -- and its
    /// [`tuple_element_types`] source for per-element types, so a nested tuple keeps resolved element types all the
    /// way down and falls back to [`IncanType::Unknown`] per slot only where the resolved type is not a tuple of
    /// the right arity.
    ///
    /// Each element is read through [`Self::ownership_fact_for_place`], exactly as
    /// [`Self::lower_tuple_unpack`] reads its own elements, so a non-Copy element borrows rather than moving out of
    /// a place v0 does not track partial-move state for. Each bound name becomes a real
    /// [`bir::LocalOrigin::UserBinding`] local in `loop_scope`, seeded with its own last-use countdown over the
    /// loop body, so [`Self::insert_scope_drops`] gives every non-Copy binding an explicit per-iteration drop.
    ///
    /// [`unsupported_for_pattern`] has already rejected every shape outside the accepted subset -- and every item
    /// type that is not a tuple of matching arity -- before [`Self::lower_for`] reaches this walk, so the remaining
    /// arms are unreachable in practice; they emit nothing rather than panicking if that invariant is ever violated
    /// by a hand-built AST.
    pub(super) fn bind_for_pattern_fields(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        expected_ty: &IncanType,
        place: &bir::Place,
        loop_scope: bir::ScopeId,
        reads: &dyn Fn(&str) -> usize,
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(pattern.span);
        match &pattern.node {
            ast::Pattern::Wildcard => {}
            ast::Pattern::Binding(name) => {
                let (fact, last_use) = self.ownership_fact_for_place(place, expected_ty);
                let element = bir::Operand::place(place.clone(), fact, last_use);
                let total_reads = reads(name);
                let local =
                    self.declare_new_local_with_reads(name.clone(), expected_ty.clone(), loop_scope, span, total_reads);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(local),
                        rvalue: bir::Rvalue::Use(element),
                    },
                    span,
                });
            }
            ast::Pattern::Tuple(items) => {
                let element_types = tuple_element_types(expected_ty, items.len());
                for (index, (item, element_ty)) in items.iter().zip(&element_types).enumerate() {
                    let mut field_place = place.clone();
                    field_place
                        .projection
                        .push(bir::PlaceElem::synthetic_field(index.to_string()));
                    self.bind_for_pattern_fields(item, element_ty, &field_place, loop_scope, reads, out);
                }
            }
            ast::Pattern::Literal(_) | ast::Pattern::Constructor(..) | ast::Pattern::Group(_) | ast::Pattern::Or(_) => {
            }
        }
    }

    /// Lower one general (non-range) iteration: materialize an iterator from `iter_expr` before the loop, then push
    /// a single [`bir::StatementKind::Loop`] whose body opens with a [`bir::StatementKind::IterNext`] writing each
    /// produced item into `pattern_local`, followed by `body_fn`. Shared by [`Self::lower_for`]'s general-iterable
    /// path and [`Self::lower_comprehension_clauses`]'s `for`-clause handling, so builtin-vs-protocol iteration is
    /// resolved in exactly one place rather than twice.
    ///
    /// Looks up [`TypeCheckInfo::protocol_iteration`] at `iter_expr`'s span to decide the [`bir::IterProtocol`]:
    /// `None` means a builtin collection or range, where "the iterator" is modeled as the iterable's own value (no
    /// method dispatch) -- a plain `Assign`; `Some` means a resolved `__iter__`/`__next__` protocol, where the
    /// iterator is obtained via an explicit `iter_method` [`bir::Callee::Method`] call. When the resolved protocol
    /// is fallible (`for item in iterable?:`, RFC 115), `iter_expr` is itself `ast::Expr::Try(inner)` with the `?`
    /// acting as the fallible-poll marker rather than an ordinary `Result` unwrap -- `inner` is lowered directly as
    /// the iterable in that case (matching the existing Rust-emission backend's own `(Expr::Try(inner), Some(_)) =>
    /// lower inner` special case in `src/backend/ir/lower/stmt.rs`), so the marker `?` is not double-lowered through
    /// [`Self::lower_try`]. Any other `Expr::Try` (an ordinary `for item in result_of_iterable?:` unwrap) falls
    /// through to the normal expression-lowering path, which already turns it into a
    /// [`bir::StatementKind::TryPropagate`] ahead of the loop via [`Self::lower_expr_to_place`]'s existing
    /// `Expr::Try` handling -- no special-casing needed for that form.
    ///
    /// The iterable is always read as a [`bir::OwnershipFact::Borrow`], matching
    /// [`Self::lower_method_call`]'s established receiver-borrow precedent (never an unsound move, and consistent
    /// with obtaining an iterator conceptually borrowing its source rather than consuming it at this normalized
    /// level); the materialized iterator local is polled with [`bir::OwnershipFact::MutBorrow`] each iteration,
    /// since polling advances its internal state.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_general_iteration(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        pattern_local: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        let protocol = self.type_info.protocol_iteration(iter_expr.span).cloned();
        let fallible = protocol.as_ref().is_some_and(|p| p.fallible_error_type.is_some());
        let effective_iter_expr: &ast::Spanned<ast::Expr> = match (&iter_expr.node, fallible) {
            (ast::Expr::Try(inner), true) => inner,
            _ => iter_expr,
        };

        let iterable_place = self.lower_expr_to_place(effective_iter_expr, outer_scope, out);
        let iterator_ty = match &protocol {
            Some(p) => semantic_type_from_resolved(&p.iterator_type),
            None => self.resolve_ty(effective_iter_expr.span),
        };
        let iterator_local = self.new_temp(iterator_ty, outer_scope, span);
        match &protocol {
            Some(p) => out.push(bir::Statement {
                kind: bir::StatementKind::Call {
                    destination: Some(bir::Place::from_local(iterator_local)),
                    callee: bir::Callee::Method(bir::MethodTarget::synthesized(p.iter_method.clone())),
                    args: fixed_elements(vec![bir::Operand::place(
                        iterable_place,
                        bir::OwnershipFact::Borrow,
                        false,
                    )]),
                    may_panic: false,
                },
                span,
            }),
            None => out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(iterator_local),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(iterable_place, bir::OwnershipFact::Borrow, false)),
                },
                span,
            }),
        }

        self.loop_break_targets.push(None);
        let mut body_stmts = Vec::new();

        let iter_protocol = match &protocol {
            Some(p) => bir::IterProtocol::UserDefined {
                next_method: p.next_method.clone(),
                fallible,
            },
            None => bir::IterProtocol::Builtin,
        };
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::IterNext {
                destination: bir::Place::from_local(pattern_local),
                iterator: bir::Operand::place(
                    bir::Place::from_local(iterator_local),
                    bir::OwnershipFact::MutBorrow,
                    false,
                ),
                protocol: iter_protocol,
            },
            span,
        });

        body_fn(self, loop_scope, &mut body_stmts);
        self.loop_break_targets.pop();

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
}
