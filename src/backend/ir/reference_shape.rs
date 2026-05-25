//! Predicates for IR expressions that already emit Rust reference-shaped values.
//!
//! Ownership and coercion planning may still see these expressions as ordinary Incan surface types. Keep the
//! reference-shape predicate here so conversions, method emission, and argument planning do not drift.

use super::expr::{IrExpr, IrExprKind};
use super::types::IrType;

/// Return whether an IR type is already represented as a Rust reference-like value.
#[must_use]
pub fn type_has_rust_reference_shape(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr
    )
}

/// Return whether an expression already emits a Rust reference-shaped value despite carrying an owned Incan surface
/// type in IR.
#[must_use]
pub fn expr_has_rust_reference_shape(expr: &IrExpr) -> bool {
    if type_has_rust_reference_shape(&expr.ty) {
        return true;
    }
    matches!(
        &expr.kind,
        IrExprKind::MethodCall {
            method,
            args,
            ..
        } if args.is_empty() && matches!(method.as_str(), "as_slice" | "as_str")
    )
}
