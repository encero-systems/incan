//! Lowering a declared parameter default into the form a direct consumer can supply for an omitted argument.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower one source-declared default into a deferred Body-IR computation.
    ///
    /// The ordinary function body may not contain this computation: source defaults run only when the matching
    /// parameter is omitted. While lowering it, callable-local bindings are hidden because the legacy path
    /// materializes source defaults while assembling call arguments, before the callee frame is bound. A default
    /// therefore becomes a closed Body-IR computation or a tagged refusal: a callable-local or other external
    /// source read, every explicitly unsupported Body-IR form, and a default without a usable canonical type fact
    /// refuse at the default expression's own span. The final condition is deliberately fail-closed: Body IR may
    /// not make an unchecked source default executable by reconstructing source semantics. This leaves a direct
    /// consumer no reason to consult AST/HIR/typechecker state or legacy execution.
    pub(super) fn lower_callable_default(
        &mut self,
        default_expr: Option<&ast::Spanned<ast::Expr>>,
        scope: bir::ScopeId,
    ) -> bir::CallableParamDefault {
        let Some(default_expr) = default_expr else {
            return bir::CallableParamDefault::Required;
        };

        let locals_len = self.locals.len();
        let scopes_len = self.scopes.len();
        let runtime_requirements_len = self.runtime_requirements.len();
        let panic_facts_len = self.panic_facts.len();
        let next_local = self.next_local;
        let next_scope = self.next_scope;
        let saved_remaining_reads = self.remaining_reads.clone();
        let saved_moved_out = self.moved_out.clone();
        let saved_materialized_range_locals = self.materialized_range_locals.clone();
        let saved_bindings = std::mem::take(&mut self.bindings);
        let saved_identity_bindings = std::mem::take(&mut self.identity_bindings);
        let saved_external_locals = std::mem::take(&mut self.external_locals);
        let mut stmts = Vec::new();
        let result = self.lower_expr_to_operand(default_expr, scope, &mut stmts);
        let mut unresolved_names: Vec<String> = self.external_locals.keys().cloned().collect();
        unresolved_names.sort();
        self.bindings = saved_bindings;
        self.identity_bindings = saved_identity_bindings;
        self.external_locals = saved_external_locals;

        let refusal = first_unsupported_default_statement(&stmts)
            .or_else(|| {
                (!unresolved_names.is_empty()).then(|| {
                    (
                        hir_span(default_expr.span),
                        format!(
                            "default reads Body-IR-external name(s): {}",
                            unresolved_names.join(", ")
                        ),
                    )
                })
            })
            .or_else(|| {
                self.type_info
                    .validated_newtype_coercion(default_expr.span)
                    .is_some()
                    .then(|| {
                        (
                            hir_span(default_expr.span),
                            "default requires a validated-newtype coercion Body IR does not yet represent".to_string(),
                        )
                    })
            })
            .or_else(|| {
                matches!(
                    self.resolve_ty(default_expr.span),
                    IncanType::Unknown | IncanType::Never
                )
                .then(|| {
                    (
                        hir_span(default_expr.span),
                        "default expression lacks a usable typecheck fact".to_string(),
                    )
                })
            });
        if let Some((span, description)) = refusal {
            self.locals.truncate(locals_len);
            self.scopes.truncate(scopes_len);
            self.runtime_requirements.truncate(runtime_requirements_len);
            self.panic_facts.truncate(panic_facts_len);
            self.next_local = next_local;
            self.next_scope = next_scope;
            self.remaining_reads = saved_remaining_reads;
            self.moved_out = saved_moved_out;
            self.materialized_range_locals = saved_materialized_range_locals;
            return bir::CallableParamDefault::Unsupported { span, description };
        }

        self.materialized_range_locals = saved_materialized_range_locals;
        bir::CallableParamDefault::Source(bir::DefaultComputation {
            span: hir_span(default_expr.span),
            stmts,
            result,
        })
    }

    // ---- Expressions ----
}

/// Return the first explicitly unsupported default statement, preserving the source span a direct consumer must
/// show when it refuses an omitted argument.
///
/// [`BodyBuilder::unsupported_operand`] records every unsupported expression as a
/// [`bir::StatementKind::Unsupported`] statement. Defaults can also nest executable statement sequences inside
/// control-flow, race arms, closures, generators, and match arms, so the scan walks each such sequence before the
/// deferred computation becomes callable metadata.
pub(super) fn first_unsupported_default_statement(stmts: &[bir::Statement]) -> Option<(HirSourceSpan, String)> {
    stmts.iter().find_map(first_unsupported_default_statement_inner)
}
/// Inspect one statement and each rvalue shape that owns a nested executable statement sequence.
pub(super) fn first_unsupported_default_statement_inner(statement: &bir::Statement) -> Option<(HirSourceSpan, String)> {
    match &statement.kind {
        bir::StatementKind::Unsupported { description } => Some((statement.span, description.clone())),
        bir::StatementKind::Assign { rvalue, .. } => first_unsupported_default_rvalue(rvalue),
        bir::StatementKind::If {
            then_block, else_block, ..
        } => first_unsupported_default_statement(&then_block.stmts).or_else(|| {
            else_block
                .as_ref()
                .and_then(|block| first_unsupported_default_statement(&block.stmts))
        }),
        bir::StatementKind::Loop { body } => first_unsupported_default_statement(&body.stmts),
        bir::StatementKind::Race { arms, .. } => arms
            .iter()
            .find_map(|arm| first_unsupported_default_statement(&arm.body.stmts)),
        _ => None,
    }
}
/// Inspect an rvalue's deferred executable parts without treating its explicit operands as source syntax to rebuild.
pub(super) fn first_unsupported_default_rvalue(rvalue: &bir::Rvalue) -> Option<(HirSourceSpan, String)> {
    match rvalue {
        bir::Rvalue::Closure { body, .. } => first_unsupported_default_statement(&body.stmts),
        bir::Rvalue::Generator { body, .. } => first_unsupported_default_statement(&body.stmts),
        bir::Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
            first_unsupported_default_statement(&arm.guard_stmts)
                .or_else(|| first_unsupported_default_statement(&arm.body_stmts))
        }),
        _ => None,
    }
}
