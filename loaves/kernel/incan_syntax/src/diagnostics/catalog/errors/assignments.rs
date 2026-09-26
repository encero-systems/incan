//! Errors about assignments with several targets.

use crate::ast::Span;
use crate::diagnostics::CompileError;

/// Report a chained assignment whose targets have incompatible types and whose value's type is not fully known (#1806).
///
/// A value such as `make_empty()` returning `list[_]` would have to take one type for every target, and targets of
/// incompatible types (`list[int]` and `list[str]`, unlike `list[int]` and `list[i64]`) leave none to give it. A value
/// built only from literals and builtin empty constructors is evaluated once per target instead and never reaches
/// this refusal.
pub fn chained_assignment_value_has_no_one_type(
    first_target_ty: &str,
    other_target_ty: &str,
    value_ty: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "The targets of this chained assignment have incompatible types ('{first_target_ty}' and \
             '{other_target_ty}'), and the value's type '{value_ty}' is not fully known, so it has no one type to take"
        ),
        span,
    )
    .with_hint("Assign each target in its own statement")
}
