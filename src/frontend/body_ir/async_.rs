//! Lowering for `await` and `race` suspension points.

use super::control_flow::intersect_range_layouts;
use super::reads::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower `await expr` into a [`bir::StatementKind::Await`] suspension point.
    ///
    /// The awaited operand is read through the ordinary ownership path, so the suspension carries the same
    /// [`bir::OwnershipFact`]/last-use discipline as any other read. The resumed value lands in a fresh temporary,
    /// which is what makes the suspension's destination explicit rather than implied by the surrounding statement.
    ///
    /// Records [`AbiV0RuntimeRequirement::AsyncRuntime`] on the enclosing body so a consumer reads the requirement
    /// off the body it applies to instead of re-deriving it from the program's imports and declaration modifiers.
    pub(super) fn lower_await(
        &mut self,
        awaited: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let operand = self.lower_expr_to_operand(awaited, scope, out);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::AsyncRuntime);
        let ty = self.resolve_ty(span);
        let destination = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Await {
                destination: Some(bir::Place::from_local(destination)),
                awaited: operand,
            },
            span: hir_span_value,
        });
        self.temp_operand(destination, &ty)
    }

    /// Lower `race for value:` into a [`bir::StatementKind::Race`].
    ///
    /// Each arm's awaitable is lowered into the enclosing block *before* any arm body, which is what makes "every
    /// awaitable is evaluated before selection" observable in the statement sequence rather than a claim in prose.
    /// Each arm then gets its own scope and type-refined local. Those locals retain the one canonical identity and
    /// exact token span of the shared header binding, so different arm types do not invent different source objects.
    ///
    /// An arm body containing an unsupported construct keeps its own `Unsupported` node *inside* the represented
    /// race rather than collapsing the whole expression, so a consumer loses only the construct it cannot handle.
    pub(super) fn lower_race_for(
        &mut self,
        race: &ast::RaceForExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);

        // Selection observes every awaitable, so all of them are evaluated first, in source order, into the
        // enclosing block. Only the winning arm's body runs, so arm bodies are lowered into their own blocks below.
        let mut awaitables = Vec::with_capacity(race.arms.len());
        for arm in &race.arms {
            awaitables.push(self.lower_expr_to_operand(&arm.awaitable, scope, out));
        }
        self.record_runtime_requirement(AbiV0RuntimeRequirement::AsyncRuntime);

        let range_layouts_before_arms = self.materialized_range_locals.clone();
        let mut range_layouts_after_arms = Vec::with_capacity(race.arms.len());
        let mut arms = Vec::with_capacity(race.arms.len());
        for (arm, awaitable) in race.arms.iter().zip(awaitables) {
            // A race resumes exactly one arm. Each arm therefore begins from the same predecessor provenance;
            // otherwise a source-local range constructed in a prior textual arm could authorize a projection in
            // a later arm that can execute instead.
            self.materialized_range_locals = range_layouts_before_arms.clone();
            let arm_scope = self.new_scope(Some(scope), hir_span_value);
            // The arm binds the *awaited output* type, which only the typechecker computes: `Awaitable[T]` binds
            // `T`, `JoinHandle[T]` binds `Result[T, TaskJoinError]`. The awaitable's own type would be wrong.
            let binding_ty = self
                .type_info
                .race_arm_binding_type(arm.awaitable.span)
                .map(semantic_type_from_resolved)
                .unwrap_or(IncanType::Unknown);
            // Snapshot the whole binding environment before the arm, not just the shared race binding. A block arm
            // lowers ordinary statements, and every `x = ...` in it declares a local that
            // `declare_new_local_with_reads` installs into `self.bindings`. Restoring only `race.binding`
            // would leave those arm-locals visible to later arms and to code after the race, so a trailing
            // read of a name an arm happened to shadow would silently resolve to the arm's local.
            // `insert_scope_drops` handles the *drop* obligation; it does not touch name resolution, which
            // is what this restores.
            let enclosing_bindings = self.bindings.clone();
            let enclosing_identity_bindings = self.identity_bindings.clone();
            let reads = match &arm.body {
                ast::RaceForBody::Expr(expr) => count_reads_in_expr(&race.binding.node, &expr.node),
                ast::RaceForBody::Block(stmts) => count_reads_in_stmts(&race.binding.node, stmts),
            };
            let binding = self.declare_new_local_with_reads(
                race.binding.node.clone(),
                binding_ty,
                arm_scope,
                hir_span(race.binding.span),
                reads,
            );

            let mut arm_stmts = Vec::new();
            let result = match &arm.body {
                ast::RaceForBody::Expr(expr) => self.lower_expr_to_operand(expr, arm_scope, &mut arm_stmts),
                ast::RaceForBody::Block(stmts) => {
                    self.lower_race_arm_block(stmts, arm.awaitable.span, arm_scope, &mut arm_stmts)
                }
            };
            self.insert_scope_drops(&mut arm_stmts, arm_scope);

            // Every name an arm bound -- its winner binding and any local its block body declared -- is scoped to
            // that arm, exactly like a closure body's. Code after the race, and each later arm, must keep resolving
            // every name to whatever it meant outside.
            self.bindings = enclosing_bindings;
            self.identity_bindings = enclosing_identity_bindings;
            range_layouts_after_arms.push(self.materialized_range_locals.clone());

            arms.push(bir::RaceArm {
                awaitable,
                binding,
                body: bir::Block {
                    scope: arm_scope,
                    stmts: arm_stmts,
                },
                result,
            });
        }
        // Code after the race follows exactly one selected arm. A range layout is usable only if it survives
        // every possible winner; an empty arm set conservatively carries no constructed-layout fact.
        self.materialized_range_locals = intersect_range_layouts(range_layouts_after_arms);

        let ty = self.resolve_ty(span);
        let destination = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Race {
                destination: Some(bir::Place::from_local(destination)),
                arms,
            },
            span: hir_span_value,
        });
        self.temp_operand(destination, &ty)
    }

    /// Lower a race arm's block body, whose value is its trailing expression statement.
    ///
    /// That trailing-expression convention is the source contract the typechecker already applies, so lowering
    /// matches `check_race_arm_block_body` exactly, including its two non-expression cases: an empty block and a
    /// block whose last statement is not an expression both produce `Unit`, the same type the checker assigns them.
    /// Refusing either would make a program the source language accepts unrepresentable, and the established
    /// precedent for a valueless block arm is [`Self::lower_match`]'s own block body.
    pub(super) fn lower_race_arm_block(
        &mut self,
        stmts: &[ast::Spanned<ast::Statement>],
        arm_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let Some((last, leading)) = stmts.split_last() else {
            return bir::Operand::Constant(bir::Constant::Unit);
        };
        for (index, stmt) in leading.iter().enumerate() {
            self.lower_stmt_into(stmt, &stmts[index + 1..], scope, out);
        }
        match &last.node {
            ast::Statement::Expr(expr) => self.lower_expr_to_operand(expr, scope, out),
            _ => {
                let _ = arm_span;
                self.lower_stmt_into(last, &[], scope, out);
                bir::Operand::Constant(bir::Constant::Unit)
            }
        }
    }

    // ---- Closures and partial callables (#1101 bucket B4) ----
}
