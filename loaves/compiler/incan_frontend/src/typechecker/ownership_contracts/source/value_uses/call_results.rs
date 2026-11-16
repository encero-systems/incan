//! Current-source call-result observations shared by invocation and constructor contexts.

use incan_lang::lang::types::collections::{self, CollectionTypeId};

use super::super::super::InputCallKind;
use super::super::Builder;
use crate::ast::{CallArg, Expr, Span, Spanned};
use crate::symbols::ResolvedType;

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
        let Some(outer) = self.checker.type_info.protocols.explicit_input_calls.get(&invocation) else {
            return None;
        };
        if outer.owner.as_ref() != Some(&self.owner)
            || outer.callee != crate::symbols::canonical_builtin_member_identity("Result", "map_err")
            || outer.receiver != Some(crate::ast::Receiver::Immutable)
            || self.checker.type_info.resolved_builtin_method_call(invocation)
                != Some(incan_lang::lang::builtins::BuiltinMethodId::Result(
                    incan_lang::lang::surface::result_methods::ResultMethodId::MapErr,
                ))
        {
            return None;
        }
        let [CallArg::Positional(callback) | CallArg::Named(_, callback)] = arguments else {
            return None;
        };
        let [parameter] = outer.parameters.as_slice() else {
            return None;
        };
        if !matches!(&parameter.ty, ResolvedType::Function(parameters, _) if parameters.len() == 1)
            || !matches!(callback.node, Expr::Closure(..))
            || self.checker.type_info.expr_type(callback.span) != Some(&parameter.ty)
            || !self
                .checker
                .closure_captures(callback.span)
                .is_some_and(|captures| captures.is_empty())
            || !matches!(value.node, Expr::Call(..))
        {
            return None;
        }
        let Some(inner) = self.checker.type_info.protocols.explicit_input_calls.get(&value.span) else {
            return None;
        };
        (inner.kind == InputCallKind::Ordinary
            && inner.owner.as_ref() == Some(&self.owner)
            && matches!(
                inner.callee.origin,
                incan_semantics_core::SymbolOrigin::Module(_) | incan_semantics_core::SymbolOrigin::Package { .. }
            )
            && self
                .checker
                .type_info
                .selected_input_contract(inner)
                .is_ok_and(|contract| contract.is_none())
            && matches!(self.checker.type_info.expr_type(value.span),
                Some(ty @ ResolvedType::Generic(name, arguments))
                    if collections::from_str(name) == Some(CollectionTypeId::Result)
                        && arguments.len() == 2
                        && !self.checker.type_holds_borrow(ty)
                        && !self.checker.type_holds_native_loan(ty))
            && self
                .checker
                .native_result_dependencies(value.span)
                .is_ok_and(|dependencies| dependencies.is_none())
            && !self.checker.native_result_is_independent(value.span)
            && !self.checked_clone_call(value.span))
        .then_some(callback.span)
    }
}
