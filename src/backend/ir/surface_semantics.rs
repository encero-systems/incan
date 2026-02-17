//! Shared helpers for lowering surface-semantics features into IR.
//!
//! This module keeps soft-keyword desugaring logic centralized so lowering stays declarative and new keyword families
//! can plug into one place.

use super::expr::{BinOp, IrExprKind, TypedExpr, UnaryOp};
use super::types::IrType;
use incan_semantics_core::{AssertShape, SurfaceCallTarget};

use crate::semantics_registry::semantics_registry;

/// Result of `assert` statement decomposition before conversion to `IrExprKind::Call`.
#[derive(Debug, Clone)]
pub struct AssertCallDesugar {
    pub local_name: &'static str,
    pub canonical_path: Vec<String>,
    pub args: Vec<TypedExpr>,
}

/// Decompose `assert` statement syntax into a stdlib testing helper call.
pub fn desugar_assert_statement(condition: TypedExpr, message: Option<TypedExpr>) -> AssertCallDesugar {
    let (shape, mut args) = match &condition.kind {
        IrExprKind::BinOp {
            op: BinOp::Eq,
            left,
            right,
        } => (AssertShape::Eq, vec![left.as_ref().clone(), right.as_ref().clone()]),
        IrExprKind::BinOp {
            op: BinOp::Ne,
            left,
            right,
        } => (AssertShape::Ne, vec![left.as_ref().clone(), right.as_ref().clone()]),
        IrExprKind::UnaryOp {
            op: UnaryOp::Not,
            operand,
        } => (AssertShape::Not, vec![operand.as_ref().clone()]),
        _ => (AssertShape::Condition, vec![condition]),
    };
    if let Some(message) = message {
        args.push(message);
    }
    let target = semantics_registry()
        .assert_call_target(shape)
        .unwrap_or(SurfaceCallTarget {
            local_name: "assert",
            canonical_path: vec!["std".to_string(), "testing".to_string(), "assert".to_string()],
        });
    AssertCallDesugar {
        local_name: target.local_name,
        canonical_path: target.canonical_path,
        args,
    }
}

/// Lower `await <expr>` into an IR `Await` expression.
pub fn lower_await_expression(inner: TypedExpr) -> (IrExprKind, IrType) {
    let ty = inner.ty.clone();
    (IrExprKind::Await(Box::new(inner)), ty)
}
