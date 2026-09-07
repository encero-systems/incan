//! Lowering for `assert` forms and their explicit panic facts.

use incan_core::lang::errors as runtime_errors;

use super::match_::PatternReadScope;
use super::refusals::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower any of RFC 018's three `assert` forms into one [`bir::StatementKind::Assert`], recording a
    /// [`bir::PanicReason::AssertFailure`] panic fact and an [`AbiV0RuntimeRequirement::PanicStrategy`] runtime
    /// requirement, because every form can panic. The optional failure message applies to all three and is lowered
    /// after the form's own operands, matching source evaluation order.
    ///
    /// `remaining` is the statement suffix following this assertion in its enclosing block. Only the
    /// `assert value is P` form uses it: unlike a `match` arm, a pattern assertion binds `P`'s names for the rest
    /// of that block, so the suffix is what seeds each binding's last-use countdown (see [`PatternReadScope`]).
    ///
    /// The pattern form reuses [`Self::lower_match_pattern`] rather than approximating the binding, so `v` in
    /// `assert value is Some(v)` becomes a declared local carrying the same [`bir::PatternBinding`]
    /// ownership/last-use facts `match value: case Some(v)` would produce. Lowering deliberately does *not* restore
    /// `self.bindings` afterwards the way [`Self::lower_match`] does at an arm boundary — that persistence is the
    /// whole point of the form.
    pub(super) fn lower_assert(
        &mut self,
        assert_stmt: &ast::AssertStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let Some(kind) = self.lower_assert_kind(&assert_stmt.kind, remaining, scope, span, out) else {
            return;
        };
        let message = assert_stmt
            .message
            .as_ref()
            .map(|m| self.lower_expr_to_operand(m, scope, out));
        self.panic_facts.push(bir::PanicFact {
            span,
            reason: bir::PanicReason::AssertFailure,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assert {
                kind,
                message,
                may_panic: true,
            },
            span,
        });
    }

    /// Lower the form-specific payload of one assertion, or push an explicit refusal and return `None`.
    ///
    /// Each refusal names the form it hit, so a placeholder left in a lowered body says which assertion spelling
    /// lowering could not represent instead of lumping two unrelated forms under one label. Both refusals are
    /// decided *before* any of the form's operands are lowered — the same "check before partially lowering"
    /// precedent [`Self::lower_match`] and [`Self::lower_binary`] follow — so a refused assertion never leaves
    /// half-lowered reads behind it.
    fn lower_assert_kind(
        &mut self,
        kind: &ast::AssertKind,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::AssertionKind> {
        match kind {
            ast::AssertKind::Condition(cond_expr) => {
                let cond = self.lower_expr_to_operand(cond_expr, scope, out);
                Some(bir::AssertionKind::Condition { cond })
            }
            ast::AssertKind::IsPattern { value, pattern } => {
                // The same rule `lower_match` applies to every arm pattern, for the same reason: a byte-string
                // literal is the one shape `bir::Constant` cannot represent. RFC 018's parser only ever produces
                // `Some`/`Ok`/`Err` with a single binding or `_`, plus bare `None`, so this is unreachable from
                // real source and stands as defence in depth for a hand-built AST -- the same standing
                // `unsupported_for_pattern`'s own type-agreement check has.
                if !match_pattern_is_supported(&pattern.node) {
                    self.push_unsupported_stmt(
                        "assert `is` form with a byte-string literal pattern".to_string(),
                        span,
                        out,
                    );
                    return None;
                }

                let scrutinee_ty = self.resolve_ty(value.span);
                let scrutinee_place = self.lower_expr_to_place(value, scope, out);
                // Read the whole scrutinee as `Borrow`, exactly as `lower_match` does and for the same reason: the
                // overall read must not risk an unconditional move while the pattern's own bindings compute more
                // precise facts against places projected out of it.
                let scrutinee = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

                // Binding into `scope` rather than a fresh child scope is what makes the names outlive the
                // assertion, and leaving `saved_bindings` unrestored is what keeps them visible to the statements
                // that follow. `seen` still dedupes repeated names within this one pattern.
                let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
                let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
                let pattern = self.lower_match_pattern(
                    pattern,
                    &scrutinee_ty,
                    &scrutinee_place,
                    scope,
                    &PatternReadScope::FollowingStatements(remaining),
                    &mut seen,
                    &mut saved_bindings,
                );
                Some(bir::AssertionKind::Pattern {
                    scrutinee,
                    pattern: Box::new(pattern),
                })
            }
            ast::AssertKind::Raises { call, error_type } => {
                let Some(expected_error) = expected_runtime_error(&error_type.node) else {
                    self.push_unsupported_stmt(
                        format!(
                            "assert `raises` form with an unresolved error type `{}`",
                            error_type.node
                        ),
                        span,
                        out,
                    );
                    return None;
                };
                let call = self.lower_expr_to_operand(call, scope, out);
                Some(bir::AssertionKind::Raises { call, expected_error })
            }
        }
    }

    // ---- Callable defaults ----
}

/// Resolve the type named after `raises` to its builtin-exception identity, or `None` when it is not one.
///
/// Body IR stores the resolved [`incan_core::errors::ErrorKind`] instead of the source spelling, so a consumer
/// never has to re-resolve a name against the exception registry to know which error an assertion expects. The
/// accepted set is exactly the registry the typechecker validates this position against (its own
/// `check_assert_stmt`), so the two stages cannot disagree about which `raises` spellings are meaningful. A
/// non-simple type spelling, or a name outside the registry, yields `None` and refuses; the typechecker has
/// already reported such a program as an unknown symbol, so this only decides how lowering represents a body it
/// was asked to lower anyway.
fn expected_runtime_error(error_type: &ast::Type) -> Option<incan_core::errors::ErrorKind> {
    match error_type {
        ast::Type::Simple(name) => runtime_errors::from_str(name),
        _ => None,
    }
}
