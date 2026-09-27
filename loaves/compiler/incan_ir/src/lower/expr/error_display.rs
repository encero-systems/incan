//! Display of `Error` adopters that define no `__str__` (#1778).
//!
//! A model or class has a Rust `Display` only through `__str__`. An `Error` adopter carries its human-readable text in
//! `message()` instead, so the typechecker records every display operand of such a type -- an f-string `{value}` part,
//! the argument of `str(value)`, and each `print`/`println` argument, whichever way the builtin is spelled -- together
//! with the `message()` call it resolved for it. Lowering hands the emitter that call in the operand's place, a `str`,
//! which every display position already renders.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, MethodCallArgPolicy};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::ast;

/// The `Error` method whose text a displayed adopter renders.
const ERROR_MESSAGE_METHOD: &str = "message";

impl AstLowering {
    /// Replace one display operand with its `message()` call when the typechecker recorded that it renders that way.
    ///
    /// Inside a trait default method, `{self}` is recorded against the trait's `Self`; the default is expanded into
    /// each adopter, and an adopter the checker listed as having a `Display` of its own (by the same rule it applies
    /// to any displayed value) keeps displaying itself there.
    ///
    /// The checker resolved that call exactly as a written `value.message()` is resolved, so it is lowered the same
    /// way: the recorded trait dispatch (a trait default the adopter inherits, or the `Error` bound of a type
    /// parameter) becomes the call's dispatch, and the method is spelled through
    /// [`Self::project_method_target_for_identity`] with the recorded identity -- the declaration's own name for
    /// another library's type, the provider's export for a compiled standard-library type, and the recoverable
    /// projection for a type this compilation declares. Every other operand is returned unchanged.
    pub(in crate::lower) fn display_operand_through_error_message(
        &self,
        operand: TypedExpr,
        operand_span: ast::Span,
    ) -> TypedExpr {
        let Some(display) = self
            .type_info
            .as_ref()
            .and_then(|info| info.error_message_display(operand_span))
            .cloned()
        else {
            return operand;
        };
        // `{self}` in a trait default is decided per adopter: one with a `Display` of its own displays itself.
        if display.receiver_is_trait_self
            && self
                .current_impl_type
                .as_ref()
                .is_some_and(|adopter| display.self_displaying_adopters.contains(adopter))
        {
            return operand;
        }
        let dispatch = display
            .dispatch
            .map(|dispatch| self.lower_resolved_method_dispatch(dispatch, &operand));
        let (method, dispatch) = self.project_method_target_for_identity(
            display.identity.as_ref(),
            ERROR_MESSAGE_METHOD,
            &operand,
            dispatch,
        );
        TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(operand),
                method,
                dispatch,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::String,
        )
    }
}
