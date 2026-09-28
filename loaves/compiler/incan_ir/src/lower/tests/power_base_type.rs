//! The base of `**` is handed to the emitter in the operation's concrete result type (#1811): `**` is spelled as a
//! method call on its base, and a literal base, or a binding initialized from one, would leave that receiver's numeric
//! type ambiguous.

use super::*;
use incan_lang::lang::types::numerics::NumericTypeId;

/// Return the base of the `**` the named function returns.
fn power_base<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    let function = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))?;
    let Some(IrStmtKind::Return(Some(returned))) = function.body.last().map(|stmt| &stmt.kind) else {
        return Err(format!("`{name}` must end in a return, got {:?}", function.body.last()));
    };
    match &returned.kind {
        IrExprKind::BinOp {
            op: crate::expr::BinOp::Pow,
            left,
            ..
        } => Ok(left),
        other => Err(format!("`{name}` must return a `**` expression, got {other:?}")),
    }
}

/// #1811: a literal base, a negated or parenthesized literal base, a literal-bound name and an exact-width integer
/// base are each converted to the result type; an operator-shaped base is grouped inside the conversion.
#[test]
fn power_base_is_converted_to_the_result_type_issue1811() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def literal_base() -> int:
    return 2 ** 3

def float_literal_base() -> float:
    return 2.0 ** 0.5

def negated_literal_base() -> int:
    return (-2) ** 3

def operator_base() -> int:
    return (1 + 2) ** 2

def literal_bound_name() -> int:
    base = 2
    return base ** 3

def narrow_integer_base(b: u8) -> int:
    return b ** 2

def integer_base_float_result(n: int) -> float:
    return n ** -1

def exact_float_base(s: f32) -> f32:
    return s ** s
"#,
    )?;

    for (name, result_ty, grouped) in [
        ("literal_base", IrType::Int, false),
        ("float_literal_base", IrType::Float, false),
        ("negated_literal_base", IrType::Int, false),
        ("operator_base", IrType::Int, true),
        ("literal_bound_name", IrType::Int, false),
        ("narrow_integer_base", IrType::Int, false),
        ("integer_base_float_result", IrType::Float, false),
        ("exact_float_base", IrType::Numeric(NumericTypeId::F32), false),
    ] {
        let base = power_base(&ir, name)?;
        let IrExprKind::Cast { expr, to_type } = &base.kind else {
            return Err(format!("`{name}` must convert its base, got {base:?}"));
        };
        assert_eq!(*to_type, result_ty, "`{name}` converts its base to the result type");
        assert_eq!(base.ty, result_ty, "`{name}`'s converted base carries the result type");
        assert_eq!(
            matches!(expr.kind, IrExprKind::Block { .. }),
            grouped,
            "`{name}` groups only an operator-shaped base, got {expr:?}"
        );
    }
    Ok(())
}
