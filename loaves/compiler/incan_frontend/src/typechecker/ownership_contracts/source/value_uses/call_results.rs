//! Current-source call-result observations shared by invocation and constructor contexts.

use super::super::Builder;
use crate::ast::{CallArg, Expr, Span, Spanned};

impl Builder<'_> {
    /// Authenticate one direct dependency-free source `Result` call as the temporary receiver of the exact
    /// selected capture-free builtin `Result.map_err` call. This remains a scoped call-result observation: carrier
    /// constructors, returned views, stored Results and captureful callbacks do not become generally independent.
    pub(super) fn closed_source_result_receiver(
        &self,
        value: &Spanned<Expr>,
        invocation: Span,
        arguments: &[CallArg],
    ) -> Option<Span> {
        let selected = super::super::call_results::selected_result_map_err_surface(
            &self.checker.type_info,
            &self.owner,
            value,
            invocation,
            arguments,
        )?;
        if !self
            .checker
            .closure_captures(selected.callback)
            .is_some_and(|captures| captures.is_empty())
            || self.checker.type_holds_borrow(&selected.result)
            || self.checker.type_holds_native_loan(&selected.result)
            || self
                .checker
                .type_info
                .rust
                .checked_calls
                .contains_key(&selected.receiver)
        {
            return None;
        }
        Some(selected.callback)
    }
}
