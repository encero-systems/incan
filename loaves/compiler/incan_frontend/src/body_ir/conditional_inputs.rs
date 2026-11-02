//! Path-local moved state for canonical whole owners with authenticated explicit transfers.

use std::collections::HashSet;

use super::{BodyBuilder, ast, bir};

/// Only structural fallthrough is needed here. Ownership and exact branch topology have already been checked by
/// the source graph; this result excludes returning arms from the lexical cleanup join without evaluating a condition.
pub(super) fn reaches_join(body: &[ast::Spanned<ast::Statement>]) -> bool {
    for statement in body {
        match &statement.node {
            ast::Statement::Return(_) => return false,
            ast::Statement::If(branch) => {
                let reaches = reaches_join(&branch.then_body)
                    || branch.elif_branches.iter().any(|(_, body)| reaches_join(body))
                    || branch.else_body.as_ref().is_none_or(|body| reaches_join(body));
                if !reaches {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

impl BodyBuilder<'_, '_> {
    /// Capture subjects before lowering any branch, so a transfer in a later arm also protects an earlier arm's
    /// cleanup. Invalid or incomplete source proof supplies no authority; final body admission retains its refusal.
    pub(super) fn retain_input_transfer_subjects(&mut self, function: &ast::FunctionDecl) {
        let subjects = self
            .input_body
            .as_ref()
            .ok()
            .and_then(|proof| proof.as_ref())
            .and_then(|proof| proof.whole_transfer_subjects(function).ok());
        self.input_paths_checked = subjects.is_some();
        self.input_transfer_subjects = subjects.unwrap_or_default();
    }

    /// Include all existing canonical subject storage, including an owner already moved before this branch.
    /// A later replacement reestablishes that storage's lexical obligation on only the paths that initialize it.
    pub(super) fn input_subject_locals(&self) -> HashSet<bir::LocalId> {
        self.locals
            .iter()
            .filter(|local| {
                local
                    .identity
                    .as_ref()
                    .is_some_and(|identity| self.input_transfer_subjects.contains(identity))
            })
            .map(|local| local.id)
            .collect()
    }

    /// Snapshot only explicit-transfer subjects; unrelated ordinary locals retain their existing lowering policy.
    pub(super) fn input_move_state(&self, subjects: &HashSet<bir::LocalId>) -> HashSet<bir::LocalId> {
        subjects.intersection(&self.moved_out).copied().collect()
    }

    /// Restore one path's subject state without erasing branch-local or unrelated ordinary-owner facts.
    pub(super) fn restore_input_move_state(&mut self, subjects: &HashSet<bir::LocalId>, state: &HashSet<bir::LocalId>) {
        self.moved_out.retain(|local| !subjects.contains(local));
        self.moved_out.extend(state.iter().copied());
    }

    /// Returning arms do not constrain the state that reaches a following statement. Their own native cleanup is
    /// still required; omitting them from this join never authorizes suppressing cleanup along the return itself.
    pub(super) fn retain_reaching_input_moves(
        &self,
        body: &[ast::Spanned<ast::Statement>],
        subjects: &HashSet<bir::LocalId>,
        states: &mut Vec<HashSet<bir::LocalId>>,
    ) {
        if reaches_join(body) {
            states.push(self.input_move_state(subjects));
        }
    }

    /// Only statement-if scopes with explicit branch joins may transfer a canonical whole subject conditionally.
    /// Assertion failure messages, expression conditionals and loops keep their existing deliberate refusal.
    pub(super) fn input_transfer_path_supported(&self, local: bir::LocalId) -> bool {
        self.conditional_scopes.is_empty()
            || (self
                .locals
                .get(local.index())
                .and_then(|local| local.identity.as_ref())
                .is_some_and(|identity| self.input_transfer_subjects.contains(identity))
                && self
                    .conditional_scopes
                    .iter()
                    .all(|scope| self.input_if_scopes.contains(scope)))
    }
}

impl BodyBuilder<'_, '_> {
    /// Retain the profile the checker admitted: profile 1 for a straight-line body, profile 2 for a body with
    /// statement-ifs, which also carries its owned initializer identities for native authentication.
    pub(super) fn checked_function_input_profile(
        &mut self,
        owner: &incan_semantics_core::CanonicalSymbolId,
        function: &ast::FunctionDecl,
        params: &[bir::CallableParam],
        span: incan_semantics_core::HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::CheckedInputProfile> {
        let admitted = self.type_info.executable_function_input(owner, function);
        self.checked_input_profile(admitted, params, span, out)
    }

    /// Method frames select their profile as functions do. The receiver occupies slot 0 with its ordinary borrowed
    /// ABI; the checker proved its access from the receiver mode.
    pub(super) fn checked_method_input_profile(
        &mut self,
        owner: &incan_semantics_core::CanonicalSymbolId,
        method: &ast::MethodDecl,
        params: &[bir::CallableParam],
        span: incan_semantics_core::HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::CheckedInputProfile> {
        let admitted = self.type_info.executable_method_input(owner, method);
        self.checked_input_profile(admitted, params, span, out)
    }

    /// Publish an admitted profile over the frame's parameters, or report the checker's refusal in the body.
    fn checked_input_profile(
        &mut self,
        admitted: Result<Option<(u32, crate::typechecker::CheckedInputBody<'_>)>, &'static str>,
        params: &[bir::CallableParam],
        span: incan_semantics_core::HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::CheckedInputProfile> {
        let (version, proof) = match admitted {
            Ok(proof) => proof?,
            Err(reason) => {
                self.push_unsupported_stmt(reason.to_owned(), span, out);
                return None;
            }
        };
        Some(bir::CheckedInputProfile {
            version,
            owner: proof.canonical_owner().clone(),
            owned_initializers: if version == 2 {
                std::mem::take(&mut self.input_owned_initializers)
            } else {
                Vec::new()
            },
            parameters: params
                .iter()
                .enumerate()
                .map(|(slot, parameter)| bir::CheckedInputParameter {
                    slot,
                    access: parameter.input_access,
                })
                .collect(),
        })
    }
}
