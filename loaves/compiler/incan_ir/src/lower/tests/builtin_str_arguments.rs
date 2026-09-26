//! The `str(...)` builtin over its argument shapes: an operator expression is handed to the emitter as one grouped
//! value in its own type, an atomic argument as written (#1726).

use super::*;

/// Return the `str(...)` builtin-call arguments of the lowered program, in source order.
fn str_builtin_arguments(ir: &mut IrProgram) -> Vec<TypedExpr> {
    struct Collect(Vec<TypedExpr>);
    impl crate::visit::Visitor for Collect {
        fn expr(&mut self, expr: &mut crate::IrExpr) {
            if let IrExprKind::BuiltinCall {
                func: crate::expr::BuiltinFn::Str,
                args,
            } = &expr.kind
            {
                self.0.extend(args.iter().cloned());
            }
            crate::visit::walk_expr(expr, self);
        }
    }
    let mut collect = Collect(Vec::new());
    for decl in &mut ir.declarations {
        if let IrDeclKind::Function(function) = &mut decl.kind {
            for stmt in &mut function.body {
                crate::visit::walk_stmt(stmt, &mut collect);
            }
        }
    }
    collect.0
}

/// #1726: `str(a + b)` renders the whole sum. The conversion is a postfix method on the argument's tokens, so an
/// operator-shaped argument is handed over as one grouped value in its own type; an atomic argument is left as
/// written.
#[test]
fn str_over_an_operator_expression_lowers_a_grouped_argument_issue1726() -> Result<(), String> {
    let mut ir = lower_source(
        r#"
model Reading:
  value: int

def main() -> None:
  a = 1
  b = 2
  reading = Reading(value=10)
  println(str(a + b))
  println(str(a * 5 + b))
  println(str(reading.value + b))
  println(str(-a))
  println(str(a))
  println(str(reading.value))
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    let arguments = str_builtin_arguments(&mut ir);
    assert_eq!(arguments.len(), 6, "{arguments:?}");

    for (argument, what) in arguments
        .iter()
        .take(4)
        .zip(["a + b", "a * 5 + b", "reading.value + b", "-a"])
    {
        let IrExprKind::Block {
            stmts,
            value: Some(value),
        } = &argument.kind
        else {
            return Err(format!(
                "`str({what})` must hand the emitter a grouped value, got {argument:?}"
            ));
        };
        assert!(stmts.is_empty(), "a grouped value carries no statements: {stmts:?}");
        assert_eq!(
            argument.ty, value.ty,
            "the group keeps the argument's type for `str({what})`"
        );
        assert!(
            matches!(value.kind, IrExprKind::BinOp { .. } | IrExprKind::UnaryOp { .. }),
            "the group wraps the operator expression itself for `str({what})`: {value:?}"
        );
    }
    assert!(
        matches!(arguments[4].kind, IrExprKind::Var { .. }),
        "`str(a)` is one operand already and stays as written: {:?}",
        arguments[4]
    );
    assert!(
        matches!(arguments[5].kind, IrExprKind::Field { .. }),
        "`str(reading.value)` is one operand already and stays as written: {:?}",
        arguments[5]
    );
    Ok(())
}
