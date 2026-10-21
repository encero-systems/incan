//! Lowering for conditional and looping control flow, and the iteration protocol behind `for`.

use super::args::*;
use super::primitives::*;
use super::reads::*;
use super::refusals::*;
use super::*;

/// Where one range-shaped `for` loop takes its bounds, step, and inclusivity from.
///
/// Inline headers, materialized aggregates, and checker-proven builtin calls retain their own layout authority while
/// driving the same normalized counting loop. Each evaluates its range once, before the first iteration, as legacy's
/// native range does.
enum RangeLoopSource {
    /// An inline `start..end` / `start..=end` loop header: its start, evaluated first, and the local its end was
    /// snapshotted into.
    Header {
        start: bir::Operand,
        end: bir::LocalId,
        inclusive: bool,
    },
    /// A snapshot of a materialized [`bir::AggregateKind::Range`] value, read back through its own declared fields.
    Value(bir::Place),
    /// Checked builtin range arguments, snapshotted in source order before the first iteration.
    Builtin(Box<BuiltinRangeBounds>),
}

/// Scalar operands retained once from a checked builtin call; boxed together to keep range classification compact.
struct BuiltinRangeBounds {
    start: bir::Operand,
    end: bir::Operand,
    step: bir::Operand,
}

/// The independently retained storage or scalar state of one canonical direct zip operand.
enum ZipLoopSource {
    /// An original list and the initializer whose checked borrow mode its component's mutation settles.
    List { iterator: bir::LocalId, initializer: usize },
    /// Once-evaluated builtin bounds and the next scalar item, without materializing a range as a list.
    Range {
        bounds: Box<BuiltinRangeBounds>,
        index: bir::LocalId,
    },
}

/// Read a counting loop's index local, or the local a header's end bound was snapshotted into.
///
/// Both are compiler-owned scalar temporaries the loop re-reads several times per iteration, so their reads are always
/// plain [`bir::OwnershipFact::Copy`] and never a last use. Going through [`BodyBuilder::ownership_fact_for_place`]
/// would consult a read countdown that was never seeded for a temporary.
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
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/control_flow.rs` discards any branch value and always
    /// returns `ResolvedType::Unit`) -- unlike a `loop` expression, an `if` expression cannot yet produce a value
    /// from its branches, so its Body IR operand is always the `Unit` constant rather than a place read.
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
    /// `loop_scope` is the caller's to create rather than this helper's. A `loop:` expression declares its result in
    /// the enclosing scope before lowering its body, so that result survives every loop-body exit.
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
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut body_stmts = Vec::new();
        lower_body(self, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
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
        let in_place = self.matches_in_place(value.span, &scrutinee_ty);
        let scrutinee_place = self.lower_expr_to_place(value, scope, out);
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
            let in_place = builder.matches_in_place(value.span, &scrutinee_ty);
            let scrutinee_place = builder.lower_expr_to_place(value, loop_scope, body_stmts);
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
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/control_flow.rs`), so -- unlike an `if` expression,
    /// which is always `Unit` -- a `loop` expression's produced value genuinely comes from its branches and needs
    /// this merge-into-one-place treatment; see [`Self::lower_break`] for the other half of the mechanism.
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
    ///
    /// A literal `true` condition is the infinite loop `loop:` spells, and lowers exactly as that statement does, with
    /// no exit test: only the body's own `break` or `return` leaves it. Legacy emission makes the same decision (it
    /// emits Rust's `loop` for a literal `true`), so a value-returning body that ends in such a loop has no
    /// fallthrough.
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
    pub(super) fn lower_for(
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
    pub(super) fn for_item_type(&self, pattern: &ast::Spanned<ast::Pattern>, iter_ty: &IncanType) -> IncanType {
        let checked = self.resolve_ty(pattern.span);
        if let Some([element]) = collection_member_types(iter_ty, CollectionTypeId::List) {
            return self.refine_task_storage(&checked, element);
        }
        if !matches!(checked, IncanType::Unknown) {
            return checked;
        }
        range_value_element_type(iter_ty).cloned().unwrap_or(checked)
    }

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
    fn range_loop_source(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        iter_ty: &IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<Option<RangeLoopSource>, String> {
        let span = hir_span(iter_expr.span);
        if let Some((start, end, inclusive)) = inline_range_parts(iter_expr) {
            let start_value = self.lower_expr_to_operand(start, scope, out);
            let start = self.retain_evaluated_value_read(start, start_value, scope, out);
            let end_ty = self.resolve_ty(end.span);
            let end_operand = self.lower_expr_to_operand(end, scope, out);
            let end = self.snapshot_local(end_operand, end_ty, scope, span, out);
            return Ok(Some(RangeLoopSource::Header { start, end, inclusive }));
        }
        if let Some(range) = self.builtin_range_loop_source(iter_expr, scope, out)? {
            return Ok(Some(range));
        }
        if range_value_element_type(iter_ty).is_none() {
            return Ok(None);
        }
        let Some(local) = self.materialized_range_local(iter_expr) else {
            return Err("range value without a source-local Body IR range aggregate".to_string());
        };
        let place = bir::Place::from_local(local);
        let range_ty = self.locals[local.index()].ty.clone();
        let (fact, last_use) = self.ownership_fact_for_place(&place, &range_ty);
        let snapshot = self.snapshot_local(bir::Operand::place(place, fact, last_use), range_ty, scope, span, out);
        Ok(Some(RangeLoopSource::Value(bir::Place::from_local(snapshot))))
    }

    /// Copy a once-evaluated range bound or range value into a fresh compiler-owned local the loop re-reads.
    fn snapshot_local(
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
    /// error before any iteration. Bound range values and other iterators keep their distinct existing paths.
    fn builtin_range_loop_source(
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

    /// Select positive-step or negative-step exhaustion from the retained scalar builtin arguments.
    fn builtin_range_stop_condition(
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

    /// Return the local holding a range value whose layout provenance is still live.
    fn materialized_range_local(&self, expr: &ast::Spanned<ast::Expr>) -> Option<bir::LocalId> {
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
    fn read_range_field(&mut self, range: &bir::Place, field: &str, field_ty: &IncanType) -> bir::Operand {
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

    /// Retain the range's checked index update as operations shared by fallthrough and each `continue` edge.
    ///
    /// A source range advances by its unit step, so an index at the maximum integer has visited an inclusive range's
    /// last item: the loop ends there, as legacy's native range does, instead of advancing past it. A builtin range
    /// keeps its own runtime advancement contract.
    fn range_increment_steps(
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
    fn push_range_maximum_exit(
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
    fn lower_range_counting_loop(
        &mut self,
        for_stmt: &ast::ForStmt,
        item_ty: &IncanType,
        source: &RangeLoopSource,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
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

        let loop_scope = self.new_scope(Some(scope), span);
        // `for` never produces a value from `break` (same reasoning as `while` -- see `Self::lower_while`).
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut body_stmts = Vec::new();

        let cond = match source {
            RangeLoopSource::Header { end, inclusive, .. } => {
                let cmp_op = if *inclusive { bir::BinOp::Gt } else { bir::BinOp::Ge };
                self.push_assign_temp(
                    bir::Rvalue::BinaryOp(cmp_op, index_read(idx_local), index_read(*end)),
                    IncanType::Primitive(IncanPrimitiveType::Bool),
                    loop_scope,
                    span,
                    &mut body_stmts,
                )
            }
            RangeLoopSource::Value(range) => {
                self.range_value_stop_condition(range, idx_local, loop_scope, span, &mut body_stmts)
            }
            RangeLoopSource::Builtin(bounds) => self.builtin_range_stop_condition(
                &bounds.end,
                &bounds.step,
                idx_local,
                loop_scope,
                span,
                &mut body_stmts,
            ),
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

        let increment = self.range_increment_steps(source, idx_local, loop_scope, span);
        if let Some(actions) = self.loop_continue_actions.last_mut() {
            *actions = increment;
        }
        self.lower_block_into(&for_stmt.body, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);
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
    /// Each element is read through [`Self::ownership_fact_for_place`], exactly as [`Self::lower_tuple_unpack`] reads
    /// its own elements, so a non-Copy element borrows rather than moving out of a place v0 does not track
    /// partial-move state for. Each bound name becomes a real [`bir::LocalOrigin::UserBinding`] local in `loop_scope`,
    /// seeded with its own last-use countdown over the loop body, so [`Self::insert_scope_drops`] gives every non-Copy
    /// binding an explicit per-iteration drop.
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
                        .push(bir::PlaceElem::structural_field(index.to_string()));
                    self.bind_for_pattern_fields(item, element_ty, &field_place, loop_scope, reads, out);
                }
            }
            ast::Pattern::Literal(_) | ast::Pattern::Constructor(..) | ast::Pattern::Group(_) | ast::Pattern::Or(_) => {
            }
        }
    }

    /// Lower a general iteration through the same checked path for statement loops and comprehension clauses.
    ///
    /// The iterable is evaluated once before the loop. Builtin collections retain their borrowed-value initializer and
    /// [`bir::StatementKind::IterNext`]; a custom protocol uses separately retained canonical hook calls, intrinsic
    /// Option dispatch and, for a fallible poll, checked [`bir::StatementKind::TryPropagate`] routing. No backend
    /// resolves a user hook from its spelling.
    ///
    /// Only the checker's structural protocol record selects hook calls. Builtin iterator values keep builtin polling,
    /// including when standard trait declarations are loaded.
    ///
    /// A fallible loop's header `?` marks polling, so its inner expression is materialized without a second Result
    /// unwrap. An ordinary Result-valued iterable still goes through normal expression propagation. Hook receivers
    /// use their checked modes; a builtin loop that changes an item in place borrows the actual initializer
    /// exclusively, preserving legacy's `iter_mut` behavior even when evaluating the iterable emitted other statements.
    /// Checker-selected item-taking loops and whole temporary lists move their builtin iterable, as legacy's
    /// `into_iter` does. Other builtin sources remain borrowed, and consuming iterators are polled exclusively without
    /// changing their initialization into an in-place item borrow.
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
        // The private hook table also covers opaque SDK Iterator returns whose legacy view stays builtin.
        let custom = self
            .type_info
            .rust
            .iteration_hooks
            .contains_key(&(iter_expr.span.start, iter_expr.span.end))
            || protocol.is_some()
            || self
                .type_info
                .protocols
                .body_ir_iterations
                .contains_key(&(iter_expr.span.start, iter_expr.span.end));
        let effective_iter_expr: &ast::Spanned<ast::Expr> = match (&iter_expr.node, fallible) {
            (ast::Expr::Try(inner), true) => inner,
            _ => iter_expr,
        };

        if !custom
            && self.enumerated_pattern_is_normalizable(pattern_local)
            && let Some(source) = self.enumerated_loop_source(effective_iter_expr)
        {
            self.lower_enumerated_iteration(source, pattern_local, outer_scope, loop_scope, span, out, body_fn);
            return;
        }

        if !custom
            && self.pair_pattern_is_normalizable(pattern_local)
            && let Some(sources) = self.zipped_loop_sources(effective_iter_expr)
        {
            if let Err(reason) =
                self.lower_zipped_iteration(sources, pattern_local, outer_scope, loop_scope, span, out, body_fn)
            {
                self.push_unsupported_stmt(reason, span, out);
            }
            return;
        }
        if !custom && let Some(source) = self.dictionary_values_source(effective_iter_expr) {
            self.lower_dictionary_values_iteration(source, pattern_local, outer_scope, loop_scope, span, out, body_fn);
            return;
        }

        // ---- Materialize the iterable exactly once ----
        let (iterator_local, iterator_init, taking) =
            match self.initialize_general_iterator(effective_iter_expr, iter_expr.span, custom, outer_scope, span, out)
            {
                Ok(iterator) => iterator,
                Err(description) => {
                    self.push_unsupported_stmt(description, span, out);
                    return;
                }
            };

        // ---- Poll before each iteration body ----
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut body_stmts = Vec::new();

        if custom {
            if let Err(description) = self.lower_checked_iteration_poll(
                iter_expr.span,
                iterator_local,
                pattern_local,
                fallible,
                loop_scope,
                span,
                &mut body_stmts,
            ) {
                self.push_unsupported_stmt(description, span, &mut body_stmts);
            }
        } else {
            body_stmts.push(bir::Statement {
                kind: bir::StatementKind::IterNext {
                    destination: bir::Place::from_local(pattern_local),
                    iterator: bir::Operand::place(
                        bir::Place::from_local(iterator_local),
                        bir::OwnershipFact::MutBorrow,
                        false,
                    ),
                    protocol: bir::IterProtocol::Builtin,
                },
                span,
            });
        }

        // ---- Body control flow and scope cleanup ----
        body_fn(self, loop_scope, &mut body_stmts);
        self.retain_in_place_iteration_bindings(&mut body_stmts, pattern_local);
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        // A builtin loop whose body changes its item in place iterates the list exclusively, as legacy's `iter_mut`.
        if !custom && !taking && self.body_changes_item_in_place(&body_stmts, pattern_local) {
            borrow_iterable_exclusively(&mut out[iterator_init]);
        }

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

    /// A materialized list expression owns its items, as legacy iterates a call result, literal, or comprehension
    /// directly by value. Named places and projections require the checker's explicit consumption decision.
    pub(super) fn temporary_list_iterable(&self, place: &bir::Place, ty: &IncanType) -> bool {
        collection_member_types(ty, CollectionTypeId::List).is_some()
            && place.projection.is_empty()
            && place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .is_some_and(|local| local.name.is_none())
    }
    /// Select only a canonically checked direct enumeration of a list; a stored enumeration remains its own value.
    fn enumerated_loop_source<'expr>(
        &self,
        expr: &'expr ast::Spanned<ast::Expr>,
    ) -> Option<&'expr ast::Spanned<ast::Expr>> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.enumerated_loop_source(inner);
        }
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return None;
        };
        if self.type_info.resolved_builtin_call(expr.span) != Some(bir::BuiltinFnId::Enumerate) || !type_args.is_empty()
        {
            return None;
        }
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        if self
            .declared_slots_for_direct_call(name, callee.span, expr.span)
            .ok()?
            .builtin
            != Some(bir::BuiltinFnId::Enumerate)
        {
            return None;
        }
        let [ast::CallArg::Positional(source)] = args.as_slice() else {
            return None;
        };
        collection_member_types(&self.resolve_ty(source.span), CollectionTypeId::List)?;
        Some(source)
    }

    /// Normalize destructured enumeration only; a whole pair needs its own mixed-reference layout contract.
    fn enumerated_pattern_is_normalizable(&self, item: bir::LocalId) -> bool {
        let Some(local) = self.locals.get(item.index()) else {
            return false;
        };
        let elements = tuple_element_types(&local.ty, 2);
        local.origin == bir::LocalOrigin::Temporary
            && elements[0] == IncanType::Primitive(IncanPrimitiveType::Int)
            && !matches!(elements[1], IncanType::Unknown)
    }

    /// Poll a direct enumeration's original list and retain its index as explicit scalar state. Pattern components
    /// bind the produced element itself when changed, rather than a materialized tuple's snapshot of it.
    #[allow(clippy::too_many_arguments)]
    fn lower_enumerated_iteration(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        pattern_local: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        // ---- Source storage and enumeration state ----
        let source_ty = self.resolve_ty(source.span);
        let source_place = self.lower_expr_to_place(source, outer_scope, out);
        let iterator = self.new_temp(source_ty, outer_scope, span);
        let index_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let index = self.new_temp(index_ty.clone(), outer_scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(index),
                rvalue: bir::Rvalue::Use(bir::Operand::Constant(bir::Constant::Int(0))),
            },
            span,
        });
        let initializer = out.len();
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(iterator),
                rvalue: bir::Rvalue::Use(bir::Operand::place(source_place, bir::OwnershipFact::Borrow, false)),
            },
            span,
        });
        let pattern_span = self.locals[pattern_local.index()].span;
        let element_ty = tuple_element_types(&self.locals[pattern_local.index()].ty, 2)[1].clone();
        let element = self.new_temp(element_ty, loop_scope, pattern_span);
        let current_index = self.new_temp(index_ty, loop_scope, pattern_span);

        // ---- One produced element and its pre-advance index ----
        let mut stmts = vec![
            bir::Statement {
                kind: bir::StatementKind::IterNext {
                    destination: bir::Place::from_local(element),
                    iterator: bir::Operand::place(
                        bir::Place::from_local(iterator),
                        bir::OwnershipFact::MutBorrow,
                        false,
                    ),
                    protocol: bir::IterProtocol::Builtin,
                },
                span,
            },
            bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(current_index),
                    rvalue: bir::Rvalue::Use(index_read(index)),
                },
                span,
            },
            bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(index),
                    rvalue: bir::Rvalue::BinaryOp(
                        bir::BinOp::Add,
                        index_read(index),
                        bir::Operand::Constant(bir::Constant::Int(1)),
                    ),
                },
                span,
            },
        ];
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());

        // ---- Checked pattern bindings and source ownership ----
        body_fn(self, loop_scope, &mut stmts);
        self.remap_enumerated_bindings(&mut stmts, pattern_local, current_index, element);
        self.retain_in_place_iteration_bindings(&mut stmts, element);
        if self.body_changes_item_in_place(&stmts, element) {
            borrow_iterable_exclusively(&mut out[initializer]);
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts,
                },
            },
            span,
        });
    }

    /// Replace the destructured tuple's checked component reads with the normalized index and element places.
    fn remap_enumerated_bindings(
        &self,
        stmts: &mut [bir::Statement],
        pattern: bir::LocalId,
        index: bir::LocalId,
        element: bir::LocalId,
    ) {
        for statement in stmts {
            let bir::StatementKind::Assign {
                place: destination,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
            else {
                continue;
            };
            if read.place.local_id() != Some(pattern) {
                continue;
            }
            let Some(bir::PlaceElem::Field { name, .. }) = read.place.projection.first() else {
                continue;
            };
            let replacement = match name.as_str() {
                "0" => index,
                "1" => element,
                _ => continue,
            };
            let mut place = bir::Place::from_local(replacement);
            place.projection.extend(read.place.projection.iter().skip(1).cloned());
            read.place = place;
            if read.place.projection.is_empty()
                && destination.local_id().is_some_and(|local| {
                    !self.locals[local.index()]
                        .ty
                        .abi_v0_facts()
                        .ownership
                        .is_trivially_copy()
                })
            {
                read.fact = bir::OwnershipFact::Move;
                read.last_use = true;
            }
        }
    }

    /// Require a destructured checked pair before replacing its component reads with independent source items.
    fn pair_pattern_is_normalizable(&self, item: bir::LocalId) -> bool {
        let local = &self.locals[item.index()];
        local.origin == bir::LocalOrigin::Temporary
            && tuple_element_types(&local.ty, 2)
                .iter()
                .all(|ty| !matches!(ty, IncanType::Unknown))
    }

    /// Select canonical direct zip over lists or builtin ranges; a same-spelled source call keeps its ordinary path.
    fn zipped_loop_sources<'expr>(
        &self,
        expr: &'expr ast::Spanned<ast::Expr>,
    ) -> Option<[&'expr ast::Spanned<ast::Expr>; 2]> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.zipped_loop_sources(inner);
        }
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return None;
        };
        if self.type_info.resolved_builtin_call(expr.span) != Some(bir::BuiltinFnId::Zip) || !type_args.is_empty() {
            return None;
        }
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        if self
            .declared_slots_for_direct_call(name, callee.span, expr.span)
            .ok()?
            .builtin
            != Some(bir::BuiltinFnId::Zip)
        {
            return None;
        }
        let [ast::CallArg::Positional(left), ast::CallArg::Positional(right)] = args.as_slice() else {
            return None;
        };
        // Frozen legacy's two-range zip requires list storage it does not have; keep its existing named refusal.
        if self.checked_range_source(left) && self.checked_range_source(right) {
            return None;
        }
        for source in [left, right] {
            if self.checked_range_source(source) {
                continue;
            }
            collection_member_types(&self.resolve_ty(source.span), CollectionTypeId::List)?;
        }
        Some([left, right])
    }

    /// Recognize a checked builtin range through parentheses before choosing scalar zip state instead of list storage.
    fn checked_range_source(&self, source: &ast::Spanned<ast::Expr>) -> bool {
        if let ast::Expr::Paren(inner) = &source.node {
            return self.checked_range_source(inner);
        }
        self.type_info.resolved_builtin_call(source.span) == Some(bir::BuiltinFnId::Range)
    }

    /// Evaluate a zip operand in source order, retaining builtin range validation before the first poll.
    fn initialize_zip_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<ZipLoopSource, String> {
        if self.checked_range_source(source) {
            let Some(RangeLoopSource::Builtin(bounds)) = self.builtin_range_loop_source(source, scope, out)? else {
                return Err("unsupported checked zip range source".into());
            };
            let index = self.snapshot_local(
                bounds.start.clone(),
                IncanType::Primitive(IncanPrimitiveType::Int),
                scope,
                span,
                out,
            );
            return Ok(ZipLoopSource::Range { bounds, index });
        }
        let (iterator, initializer) = self.initialize_iteration_source(source, scope, span, out);
        Ok(ZipLoopSource::List { iterator, initializer })
    }

    /// Produce one zip component, stopping before the source body; scalar advancement belongs to body exit edges.
    fn poll_zip_source(
        &mut self,
        source: &ZipLoopSource,
        item: bir::LocalId,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        match source {
            ZipLoopSource::List { iterator, .. } => out.push(self.iteration_poll(*iterator, item, span)),
            ZipLoopSource::Range { bounds, index } => {
                let cond = self.builtin_range_stop_condition(&bounds.end, &bounds.step, *index, scope, span, out);
                let exit_scope = self.new_scope(Some(scope), span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::If {
                        cond,
                        then_block: bir::Block {
                            scope: exit_scope,
                            stmts: vec![bir::Statement {
                                kind: bir::StatementKind::Break { value: None },
                                span,
                            }],
                        },
                        else_block: None,
                    },
                    span,
                });
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(item),
                        rvalue: bir::Rvalue::Use(index_read(*index)),
                    },
                    span,
                });
            }
        }
    }

    /// Retain scalar zip advances for fallthrough and continue, after body effects as in an ordinary range loop.
    fn zip_range_advances(
        &mut self,
        sources: [&ZipLoopSource; 2],
        scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> Vec<bir::Statement> {
        let mut out = Vec::new();
        for source in sources {
            let ZipLoopSource::Range { bounds, index } = source else {
                continue;
            };
            let next = self.push_assign_temp(
                bir::Rvalue::BinaryOp(bir::BinOp::Add, index_read(*index), bounds.step.clone()),
                IncanType::Primitive(IncanPrimitiveType::Int),
                scope,
                span,
                &mut out,
            );
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(*index),
                    rvalue: bir::Rvalue::Use(next),
                },
                span,
            });
        }
        out
    }

    /// Evaluate one original iterable once and retain the initializer whose checked borrow mode the body settles.
    fn initialize_iteration_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> (bir::LocalId, usize) {
        let ty = self.resolve_ty(source.span);
        let place = self.lower_expr_to_place(source, scope, out);
        let iterator = self.new_temp(ty, scope, span);
        let initializer = out.len();
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(iterator),
                rvalue: bir::Rvalue::Use(bir::Operand::place(place, bir::OwnershipFact::Borrow, false)),
            },
            span,
        });
        (iterator, initializer)
    }

    /// Poll a retained builtin source, with exhaustion leaving the normalized loop before any body effect.
    fn iteration_poll(&self, iterator: bir::LocalId, item: bir::LocalId, span: HirSourceSpan) -> bir::Statement {
        bir::Statement {
            kind: bir::StatementKind::IterNext {
                destination: bir::Place::from_local(item),
                iterator: bir::Operand::place(bir::Place::from_local(iterator), bir::OwnershipFact::MutBorrow, false),
                protocol: bir::IterProtocol::Builtin,
            },
            span,
        }
    }

    /// Poll each direct zip operand independently and borrow only the source whose non-Copy component changes.
    #[allow(clippy::too_many_arguments)]
    fn lower_zipped_iteration(
        &mut self,
        sources: [&ast::Spanned<ast::Expr>; 2],
        pattern: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) -> Result<(), String> {
        // ---- Original source storage and independent polls ----
        let left = self.initialize_zip_source(sources[0], outer_scope, span, out)?;
        let right = self.initialize_zip_source(sources[1], outer_scope, span, out)?;
        let types = tuple_element_types(&self.locals[pattern.index()].ty, 2);
        let pattern_span = self.locals[pattern.index()].span;
        let first = self.new_temp(types[0].clone(), loop_scope, pattern_span);
        let second = self.new_temp(types[1].clone(), loop_scope, pattern_span);
        let mut stmts = Vec::new();
        self.poll_zip_source(&left, first, loop_scope, span, &mut stmts);
        self.poll_zip_source(&right, second, loop_scope, span, &mut stmts);
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        let advances = self.zip_range_advances([&left, &right], loop_scope, span);
        self.loop_continue_actions.push(advances);

        // ---- Checked component bindings and operand ownership ----
        body_fn(self, loop_scope, &mut stmts);
        self.remap_enumerated_bindings(&mut stmts, pattern, first, second);
        for (item, source) in [(first, left), (second, right)] {
            self.retain_in_place_iteration_bindings(&mut stmts, item);
            if let ZipLoopSource::List { initializer, .. } = source
                && self.body_changes_item_in_place(&stmts, item)
            {
                borrow_iterable_exclusively(&mut out[initializer]);
            }
        }
        if let Some(advances) = self.loop_continue_actions.last() {
            stmts.extend(advances.iter().cloned());
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts,
                },
            },
            span,
        });
        Ok(())
    }

    /// Recognize only the checked compiler-owned dictionary value view; a user method named values keeps its call.
    fn dictionary_values_source<'expr>(
        &self,
        expr: &'expr ast::Spanned<ast::Expr>,
    ) -> Option<&'expr ast::Spanned<ast::Expr>> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.dictionary_values_source(inner);
        }
        let ast::Expr::MethodCall(source, _, type_args, args) = &expr.node else {
            return None;
        };
        if !type_args.is_empty() || !args.is_empty() {
            return None;
        }
        let identity = self.type_info.resolved_identity(expr.span)?;
        if identity.origin != incan_semantics_core::SymbolOrigin::Builtin || identity.declaration_name != "Dict.values"
        {
            return None;
        }
        collection_member_types(&self.resolve_ty(source.span), CollectionTypeId::Dict)?;
        Some(source)
    }

    /// Poll original dictionary keys and retain the corresponding value place, rather than iterating copied values.
    #[allow(clippy::too_many_arguments)]
    fn lower_dictionary_values_iteration(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        item: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        // ---- Original map storage and its polled key ----
        let source_ty = self.resolve_ty(source.span);
        let key_ty = collection_member_types(&source_ty, CollectionTypeId::Dict)
            .map(|types| types[0].clone())
            .unwrap_or(IncanType::Unknown);
        let (iterator, initializer) = self.initialize_iteration_source(source, outer_scope, span, out);
        let key = self.new_temp(key_ty.clone(), loop_scope, span);
        let mut value = bir::Place::from_local(iterator);
        value
            .projection
            .push(bir::PlaceElem::Index(Box::new(self.temp_operand(key, &key_ty))));
        let mut stmts = vec![
            self.iteration_poll(iterator, key, span),
            bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(item),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(value, bir::OwnershipFact::Borrow, false)),
                },
                span,
            },
        ];
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());

        // ---- Checked value bindings and mutable value storage ----
        body_fn(self, loop_scope, &mut stmts);
        self.retain_in_place_iteration_bindings(&mut stmts, item);
        if self.body_changes_item_in_place(&stmts, item) {
            borrow_iterable_exclusively(&mut out[initializer]);
            borrow_iterable_exclusively(&mut stmts[1]);
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts,
                },
            },
            span,
        });
    }

    /// Retain exclusive reads for changed non-Copy pattern bindings only. Ordinary aliases declared in the body
    /// keep their existing value-copy semantics; the pattern's own source spans identify its generated bindings.
    fn retain_in_place_iteration_bindings(&self, stmts: &mut [bir::Statement], item: bir::LocalId) {
        let pattern_span = self.locals[item.index()].span;
        let changed: Vec<_> = stmts
            .iter()
            .filter_map(|statement| {
                let bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                } = &statement.kind
                else {
                    return None;
                };
                let local = place.local_id()?;
                (read.place.local_id() == Some(item)
                    && statement.span.start >= pattern_span.start
                    && statement.span.end <= pattern_span.end
                    && !self.locals[local.index()]
                        .ty
                        .abi_v0_facts()
                        .ownership
                        .is_trivially_copy()
                    && self.body_changes_item_in_place(stmts, local))
                .then_some(local)
            })
            .collect();
        for statement in stmts {
            if let bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
                && place.local_id().is_some_and(|local| changed.contains(&local))
            {
                read.fact = bir::OwnershipFact::MutBorrow;
                read.last_use = false;
            }
        }
    }

    /// Return whether a loop body changes its item in place: through a method that changes it, a pattern matched in
    /// place over it, a write to a part of it, or a nested loop that changes its elements in place.
    ///
    /// Legacy iterates such a loop with `iter_mut`, so each change lands in its element. An item passed to a `mut`
    /// parameter does not count: legacy passes the callee a copy of it (#1561).
    fn body_changes_item_in_place(&self, stmts: &[bir::Statement], item: bir::LocalId) -> bool {
        stmts
            .iter()
            .any(|stmt| self.statement_changes_item_in_place(stmt, item))
    }

    /// Report whether one statement, or a block nested in it, changes `item` in place.
    fn statement_changes_item_in_place(&self, statement: &bir::Statement, item: bir::LocalId) -> bool {
        match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(_),
                args,
                ..
            } => args
                .first()
                .and_then(bir::ArgumentElement::as_one)
                .is_some_and(|receiver| self.method_changes_item(receiver, statement.span, item)),
            bir::StatementKind::Assign { place, rvalue } => {
                (!place.projection.is_empty() && place.local_id() == Some(item))
                    || self.rvalue_changes_item_in_place(rvalue, item)
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                self.body_changes_item_in_place(&then_block.stmts, item)
                    || else_block
                        .as_ref()
                        .is_some_and(|block| self.body_changes_item_in_place(&block.stmts, item))
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                self.body_changes_item_in_place(&body.stmts, item)
            }
            _ => false,
        }
    }

    /// Report whether a method call at `span` changes its receiver `item`, or a part of it: a collection method that
    /// borrows it exclusively, or a source method the checker resolved to a `mut self` receiver.
    fn method_changes_item(&self, receiver: &bir::Operand, span: HirSourceSpan, item: bir::LocalId) -> bool {
        let bir::Operand::Place(read) = receiver else {
            return false;
        };
        read.place.local_id() == Some(item)
            && (read.fact == bir::OwnershipFact::MutBorrow
                || self
                    .type_info
                    .method_call_takes_mutable_receiver(ast::Span::new(span.start, span.end)))
    }

    /// Report whether an assigned value borrows `item` exclusively (a nested loop iterating it in place), or is a
    /// `match` that binds part of `item` in place or changes it in one of its arms.
    fn rvalue_changes_item_in_place(&self, rvalue: &bir::Rvalue, item: bir::LocalId) -> bool {
        match rvalue {
            bir::Rvalue::Use(operand) => borrows_item_exclusively(operand, item),
            bir::Rvalue::Match { scrutinee, arms } => {
                let over_item = matches!(scrutinee, bir::Operand::Place(read) if read.place.local_id() == Some(item));
                arms.iter().any(|arm| {
                    (over_item && pattern_binds_in_place(&arm.pattern))
                        || self.body_changes_item_in_place(&arm.guard_stmts, item)
                        || self.body_changes_item_in_place(&arm.body_stmts, item)
                })
            }
            _ => false,
        }
    }
}

/// Report whether a pattern binds any name exclusively, which marks a pattern matched in place.
fn pattern_binds_in_place(pattern: &bir::Pattern) -> bool {
    match pattern {
        bir::Pattern::Var(binding) => binding.fact == bir::OwnershipFact::MutBorrow,
        bir::Pattern::Tuple(items) | bir::Pattern::Or(items) => items.iter().any(pattern_binds_in_place),
        bir::Pattern::Enum { fields, .. }
        | bir::Pattern::Result { fields, .. }
        | bir::Pattern::UnionMember { fields, .. } => fields.iter().any(pattern_binds_in_place),
        bir::Pattern::Struct { fields, .. } | bir::Pattern::Nominal { fields, .. } => {
            fields.iter().any(|(_, field)| pattern_binds_in_place(field))
        }
        _ => false,
    }
}

/// Report whether an operand borrows `item`, or a part of it, exclusively.
fn borrows_item_exclusively(operand: &bir::Operand, item: bir::LocalId) -> bool {
    matches!(
        operand,
        bir::Operand::Place(read)
            if read.fact == bir::OwnershipFact::MutBorrow && read.place.local_id() == Some(item)
    )
}

/// Read a builtin loop's iterable exclusively, in the statement that materializes the loop's iterator from it.
fn borrow_iterable_exclusively(statement: &mut bir::Statement) {
    if let bir::StatementKind::Assign {
        rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        ..
    } = &mut statement.kind
    {
        read.fact = bir::OwnershipFact::MutBorrow;
    }
}
