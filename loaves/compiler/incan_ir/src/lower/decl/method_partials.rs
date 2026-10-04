//! Typing the forwarding call of a lowered method partial.
//!
//! A method partial (`short = partial label(prefix="name")`) lowers to a generated method whose body forwards to the
//! target method on `self`. That body is synthetic source: every node carries the owning declaration's span, so no
//! span-keyed checker fact types its receiver, and the receiver reaches the IR as `Unknown`.

use super::super::super::decl::IrFunction;
use super::super::super::expr::IrExprKind;
use super::super::super::stmt::IrStmtKind;
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_lang::lang::keywords::{self, KeywordId};

impl AstLowering {
    /// Give a lowered method partial's forwarding call the nominal type of the method's owner as its receiver type.
    ///
    /// Without the owner type the target reads as a method of an unknown foreign receiver, so its `str` arguments are
    /// passed the way a foreign method takes them rather than as the owned values the target declares (#1765). Only
    /// inherent methods of models, classes and newtypes come through here; a trait's method partial is a
    /// trait default method and is lowered with the trait.
    pub(in crate::lower) fn type_method_partial_forwarding_receiver(&self, owner: &str, function: &mut IrFunction) {
        let owner_ty = self
            .struct_names
            .get(owner)
            .or_else(|| self.enum_names.get(owner))
            .cloned()
            .unwrap_or_else(|| IrType::Struct(owner.to_string()));
        let self_name = keywords::as_str(KeywordId::SelfKw);
        for stmt in &mut function.body {
            let IrStmtKind::Return(Some(forwarded)) = &mut stmt.kind else {
                continue;
            };
            let IrExprKind::MethodCall { receiver, .. } = &mut forwarded.kind else {
                continue;
            };
            if matches!(&receiver.kind, IrExprKind::Var { name, .. } if name == self_name)
                && matches!(receiver.ty, IrType::Unknown)
            {
                receiver.ty = owner_ty.clone();
            }
        }
    }
}
