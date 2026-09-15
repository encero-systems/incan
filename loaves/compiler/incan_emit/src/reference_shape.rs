//! Predicates for IR expressions that already emit Rust reference-shaped values.
//!
//! Ownership and coercion planning may still see these expressions as ordinary Incan surface types. Keep the
//! reference-shape predicate here so conversions, method emission, and argument planning do not drift.

use incan_ir::expr::{IrExpr, IrExprKind};
use incan_ir::types::IrType;

/// Return whether an IR type is already represented as a Rust reference-like value.
#[must_use]
pub fn type_has_rust_reference_shape(ty: &IrType) -> bool {
    match ty {
        IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr => true,
        // Callback parameters from inspected Rust APIs retain their exact emitted spelling in `RustDisplay` because
        // the Incan surface type model cannot otherwise represent every borrowed Rust shape. They already evaluate
        // to references, so ownership planning must not clone or add a second borrow.
        IrType::RustDisplay(display) => display.trim_start().starts_with('&'),
        _ => false,
    }
}

/// Return whether an expression already emits a Rust reference-shaped value despite carrying an owned Incan surface
/// type in IR.
///
/// Method receivers named `self` are represented by the enclosing Rust method's `&self` or `&mut self` parameter even
/// though their source-level nominal type remains owned in IR.
#[must_use]
pub fn expr_has_rust_reference_shape(expr: &IrExpr) -> bool {
    if type_has_rust_reference_shape(&expr.ty) {
        return true;
    }
    matches!(
        &expr.kind,
        IrExprKind::Var { name, .. } if name == "self"
    ) || matches!(
        &expr.kind,
        IrExprKind::MethodCall { method, args, .. }
            if args.is_empty() && matches!(method.as_str(), "as_slice" | "as_str" | "as_ref")
    )
}

#[cfg(test)]
mod tests {
    use super::{expr_has_rust_reference_shape, type_has_rust_reference_shape};
    use incan_ir::IrType;
    use incan_ir::expr::{IrExpr, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};

    #[test]
    fn borrowed_callback_display_preserves_reference_shape() {
        assert!(type_has_rust_reference_shape(&IrType::RustDisplay(
            "&mut egui::Ui".to_string()
        )));
        assert!(type_has_rust_reference_shape(&IrType::RustDisplay(
            "&eframe::CreationContext".to_string()
        )));
        assert!(!type_has_rust_reference_shape(&IrType::RustDisplay(
            "egui::Ui".to_string()
        )));
    }

    /// Build a zero-arg `receiver.method()` call expression for the reference-shape predicate tests below.
    fn zero_arg_method_call(method: &str, receiver_ty: IrType, result_ty: IrType) -> IrExpr {
        IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "value".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    receiver_ty,
                )),
                method: method.to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            result_ty,
        )
    }

    #[test]
    fn as_ref_call_is_treated_as_already_reference_shaped() {
        // `Arc<T>`/`Box<T>`/`Rc<T>` `.as_ref()` always yields `&T`, matching the already-recognized `.as_slice()` and
        // `.as_str()` shapes; without this, argument planning wraps the result in a second `&`.
        let expr = zero_arg_method_call(
            "as_ref",
            IrType::NamedGeneric("Arc".to_string(), vec![IrType::Struct("dyn Array".to_string())]),
            IrType::Struct("dyn Array".to_string()),
        );
        assert!(expr_has_rust_reference_shape(&expr));
    }

    #[test]
    fn clone_call_is_not_treated_as_reference_shaped() {
        // `.clone()` produces an owned value, so a target expecting `&T` still needs an explicit borrow.
        let expr = zero_arg_method_call(
            "clone",
            IrType::Struct("Node".to_string()),
            IrType::Struct("Node".to_string()),
        );
        assert!(!expr_has_rust_reference_shape(&expr));
    }
}
