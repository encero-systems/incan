//! One display rule for `print`/`println` arguments, `str(value)` and f-string `{value}` parts (#1748).
//!
//! An f-string renders a tuple, list, dict, set, `Option` or `Result` through its structure (`[1, 2, 3]`,
//! `Some(1)`), because those types have no display form of their own. `print`, `println` and `str` display the same
//! values, so lowering hands the emitter each such operand as the one-part f-string `f"{value}"` would lower to: a
//! `Format` expression that yields the rendered text. Every other operand is left as written, so the three positions
//! render any value identically and the emitter needs no second rendering rule.

use super::super::super::TypedExpr;
use super::super::super::expr::{BuiltinFn, FormatPart, FormatStyle, IrExprKind, display_style_uses_structured_debug};
use super::super::super::types::IrType;
use super::super::AstLowering;

impl AstLowering {
    /// Render one `print`/`println`/`str` operand the way an f-string `{value}` part renders it.
    ///
    /// The decision is the one the f-string path makes for a `{value}` part: the operand's lowered type. A structural
    /// operand becomes the one-part f-string `f"{value}"`, a `str`, exactly as `f"{value}"` lowers; every other operand
    /// is returned unchanged and keeps its own display form. Lowering's type is authoritative here because lowering
    /// can narrow a binding further than the checker's recorded type (a union member bound by `isinstance`).
    pub(in crate::lower) fn display_operand(&self, operand: TypedExpr) -> TypedExpr {
        if !display_style_uses_structured_debug(&operand.ty) {
            return operand;
        }
        TypedExpr::new(
            IrExprKind::Format {
                parts: vec![FormatPart::Expr {
                    expr: operand,
                    style: FormatStyle::Display,
                }],
            },
            IrType::String,
        )
    }

    /// Build one compiler-owned builtin call, rendering its display operands through [`Self::display_operand`].
    ///
    /// `print`/`println` arguments are rendered one by one. `str(value)` of a structural value is the rendered text
    /// itself, so the call becomes that `f"{value}"` expression. Every other builtin keeps its arguments as lowered.
    pub(in crate::lower) fn builtin_call_with_display_operands(
        &self,
        builtin: BuiltinFn,
        args: Vec<TypedExpr>,
        result_ty: IrType,
    ) -> (IrExprKind, IrType) {
        match builtin {
            BuiltinFn::Print => {
                let args = args.into_iter().map(|arg| self.display_operand(arg)).collect();
                (IrExprKind::BuiltinCall { func: builtin, args }, result_ty)
            }
            BuiltinFn::Str => {
                let mut args = args;
                if args.len() == 1
                    && let Some(arg) = args.pop()
                {
                    let rendered = self.display_operand(arg);
                    if matches!(rendered.kind, IrExprKind::Format { .. }) {
                        return (rendered.kind, IrType::String);
                    }
                    args.push(rendered);
                }
                (IrExprKind::BuiltinCall { func: builtin, args }, result_ty)
            }
            _ => (IrExprKind::BuiltinCall { func: builtin, args }, result_ty),
        }
    }
}
