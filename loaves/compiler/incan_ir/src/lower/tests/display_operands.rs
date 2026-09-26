//! `print`/`println` arguments and `str(value)` display a tuple, list, dict, set, `Option` or `Result` through the
//! same one-part `Format` expression an f-string `{value}` part lowers to, and leave every other operand as written
//! (#1748).

use super::*;
use crate::expr::{BuiltinFn, FormatPart, FormatStyle};

/// Return the statements of the named function.
fn function_body<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [IrStmt], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function.body.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return the arguments of every `print`/`println` statement in a body, in source order.
fn printed_args(body: &[IrStmt]) -> Vec<&TypedExpr> {
    body.iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Expr(expr) => match &expr.kind {
                IrExprKind::BuiltinCall {
                    func: BuiltinFn::Print,
                    args,
                } => Some(args.iter()),
                _ => None,
            },
            _ => None,
        })
        .flatten()
        .collect()
}

/// Return the value a one-part `Format` expression displays, or explain why the expression is not one.
fn interpolated_value(expr: &TypedExpr) -> Result<&TypedExpr, String> {
    let IrExprKind::Format { parts } = &expr.kind else {
        return Err(format!("expected the one-part f-string rendering, got {:?}", expr.kind));
    };
    match parts.as_slice() {
        [
            FormatPart::Expr {
                expr: value,
                style: FormatStyle::Display,
            },
        ] if expr.ty == IrType::String => Ok(value),
        other => Err(format!("expected one display part yielding a str, got {other:?}")),
    }
}

/// Return the expression the named function returns.
fn returned_value<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    match function_body(ir, name)?.last() {
        Some(IrStmt {
            kind: IrStmtKind::Return(Some(expr)),
            ..
        }) => Ok(expr),
        other => Err(format!("`{name}` must end in a return, got {other:?}")),
    }
}

#[test]
fn print_arguments_render_structural_values_like_interpolation_issue1748() -> Result<(), String> {
    let (_, ir, _) = lower_source_with_lowering(
        r#"
def main() -> None:
    items: list[int] = [1, 2, 3]
    coords: tuple[int, int] = (10, 20)
    maybe: Option[int] = Some(1)
    count: int = 3
    print(items)
    println(coords, maybe)
    println(count)
"#,
    )?;
    let args = printed_args(function_body(&ir, "main")?);
    let [items, coords, maybe, count] = args.as_slice() else {
        return Err(format!("expected four printed arguments, got {args:?}"));
    };
    assert!(matches!(interpolated_value(items)?.ty, IrType::List(_)), "{items:?}");
    assert!(matches!(interpolated_value(coords)?.ty, IrType::Tuple(_)), "{coords:?}");
    assert!(matches!(interpolated_value(maybe)?.ty, IrType::Option(_)), "{maybe:?}");
    assert_eq!(count.ty, IrType::Int, "a scalar keeps its own display form: {count:?}");
    assert!(
        interpolated_value(count).is_err(),
        "a scalar is not rewritten: {count:?}"
    );
    Ok(())
}

#[test]
fn str_of_a_structural_value_is_its_interpolation_issue1748() -> Result<(), String> {
    let (_, ir, _) = lower_source_with_lowering(
        r#"
def render(items: list[int]) -> str:
    return str(items)

def render_count(count: int) -> str:
    return str(count)
"#,
    )?;
    let rendered = returned_value(&ir, "render")?;
    assert!(
        matches!(interpolated_value(rendered)?.ty, IrType::List(_)),
        "`str(items)` is the f-string rendering of the list: {rendered:?}"
    );
    let counted = returned_value(&ir, "render_count")?;
    assert!(
        matches!(
            &counted.kind,
            IrExprKind::BuiltinCall {
                func: BuiltinFn::Str,
                ..
            }
        ),
        "`str(count)` keeps the builtin conversion: {counted:?}"
    );
    Ok(())
}
