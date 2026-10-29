//! Select checked primitive string slice calls without changing the frozen legacy method snapshots.

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Spanned};
use crate::symbols::{ResolvedType, canonical_builtin_member_identity};
use incan_lang::lang::magic_methods::{self, MagicMethodId};

impl TypeChecker {
    /// Retain the internal slice member only for a checked primitive str receiver and exactly two positional int
    /// bounds. Missing, additional, open or non-int bounds remain unproven; source-owned methods retain their own
    /// identity. The original signed unary end is preserved rather than treated as an omitted endpoint.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_string_slice_call(
        &mut self,
        expression: &Spanned<Expr>,
        errors_before: usize,
    ) {
        if self.errors.len() != errors_before {
            return;
        }
        let Expr::MethodCall(receiver, name, type_args, arguments) = &expression.node else {
            return;
        };
        if !type_args.is_empty()
            || magic_methods::from_str(name) != Some(MagicMethodId::Slice)
            || self.body_ir_checked_value_type(receiver.span) != Some(&ResolvedType::Str)
        {
            return;
        }
        let [CallArg::Positional(start), CallArg::Positional(end)] = arguments.as_slice() else {
            return;
        };
        if self.body_ir_checked_value_type(start.span) != Some(&ResolvedType::Int)
            || self.body_ir_checked_value_type(end.span) != Some(&ResolvedType::Int)
        {
            return;
        }
        let key = expression.span;
        let canonical = canonical_builtin_member_identity("str", magic_methods::as_str(MagicMethodId::Slice));
        self.type_info.calls.body_ir_string_slice_members.insert(key, canonical);
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert(key, ResolvedType::Str);
    }
}
