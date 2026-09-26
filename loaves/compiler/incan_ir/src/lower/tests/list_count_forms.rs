//! `count` on a list lowers by argument count (#1776): `count()` counts the list's items, which is its length, and
//! `count(value)` is the list method.

use super::*;
use crate::expr::BuiltinFn;

/// Return the expression the named function returns.
fn returned_expr<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    let function = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))?;
    match function.body.last() {
        Some(IrStmt {
            kind: IrStmtKind::Return(Some(expr)),
            ..
        }) => Ok(expr),
        other => Err(format!(
            "`{name}` must end in a return of the count call, got {other:?}"
        )),
    }
}

/// #1776: `items.count()` lowers to the list's length, as `len(items)` does, for any element type (a generic element
/// that is not cloneable included), and `items.count(9)` stays the list method with its one value argument.
#[test]
fn list_count_forms_lower_by_argument_count_issue1776() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def item_count(items: list[int]) -> int:
    return items.count()

def nine_count(items: list[int]) -> int:
    return items.count(9)

def generic_count[T](items: list[T]) -> int:
    return items.count()
"#,
    )?;

    for name in ["item_count", "generic_count"] {
        match &returned_expr(&ir, name)?.kind {
            IrExprKind::BuiltinCall {
                func: BuiltinFn::Len,
                args,
            } if args.len() == 1 => {}
            other => {
                return Err(format!(
                    "`{name}`: count() must lower to the list's length, got {other:?}"
                ));
            }
        }
    }

    match &returned_expr(&ir, "nine_count")?.kind {
        IrExprKind::KnownMethodCall {
            kind: MethodKind::Collection(CollectionMethodKind::Count),
            args,
            ..
        } if args.len() == 1 => Ok(()),
        other => Err(format!("count(9) must stay the list method, got {other:?}")),
    }
}
