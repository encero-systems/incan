//! Lowering keeps the type the checker gave a literal from its destination: an integer literal in a float slot is
//! lowered as a float literal (#1831), a tuple literal carries the destination's element types (#1847), and a list
//! literal assigned to an `Option[list[...]]` binding carries the list type (#1832).

use super::*;
use crate::stmt::AssignTarget;

/// Return the statements of the named function's body.
fn body<'ir>(ir: &'ir IrProgram, name: &str) -> Result<&'ir [IrStmt], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function.body.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return the value that the `occurrence`-th statement writing the local `name` (a `let` or an assignment) writes.
fn written_value<'ir>(stmts: &'ir [IrStmt], name: &str, occurrence: usize) -> Result<&'ir TypedExpr, String> {
    stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Let { name: bound, value, .. } if bound == name => Some(value),
            IrStmtKind::Assign {
                target: AssignTarget::Var { name: assigned, .. },
                value,
            } if assigned == name => Some(value),
            _ => None,
        })
        .nth(occurrence)
        .ok_or_else(|| format!("missing write {occurrence} of `{name}`"))
}

/// Assert that `expr` is the float literal `value` of type `ty`.
fn assert_float_literal(expr: &TypedExpr, value: f64, ty: &IrType) -> Result<(), String> {
    match expr.kind {
        IrExprKind::Float(lowered) if lowered.to_bits() == value.to_bits() && &expr.ty == ty => Ok(()),
        _ => Err(format!(
            "expected the float literal {value} of type {ty:?}, got {expr:?}"
        )),
    }
}

/// #1831: an integer literal the checker typed as a float is lowered as a float literal of that type, in a
/// declaration, a reassignment, a negation and a tuple element; an integer literal in an integer slot, or as an
/// operand of `/`, stays an integer.
#[test]
fn float_typed_integer_literal_lowers_as_a_float_literal_issue1831() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def main() -> None:
    declared: float = 3
    mut f: float = 0.5
    f = 1
    mut g: f32 = 0.5
    g = -2
    floats: tuple[float, int] = (4, 5)
    n: int = 6
    ratio: float = 10 / 4
    println(declared + f + g + floats[0] + ratio)
    println(n)
"#,
    )?;
    let stmts = body(&ir, "main")?;

    assert_float_literal(written_value(stmts, "declared", 0)?, 3.0, &IrType::Float)?;
    assert_float_literal(written_value(stmts, "f", 1)?, 1.0, &IrType::Float)?;

    let f32_ty = IrType::Numeric(NumericTypeId::F32);
    let negated = written_value(stmts, "g", 1)?;
    let IrExprKind::UnaryOp {
        op: UnaryOp::Neg,
        operand,
    } = &negated.kind
    else {
        return Err(format!("`g = -2` must lower to a negation, got {negated:?}"));
    };
    assert_eq!(negated.ty, f32_ty, "the negation keeps the binding's type");
    assert_float_literal(operand, 2.0, &f32_ty)?;

    let tuple = written_value(stmts, "floats", 0)?;
    let IrExprKind::Tuple(items) = &tuple.kind else {
        return Err(format!("the tuple literal must lower to a tuple, got {tuple:?}"));
    };
    assert_eq!(tuple.ty, IrType::Tuple(vec![IrType::Float, IrType::Int]));
    let [first, second] = items.as_slice() else {
        return Err(format!("the tuple literal must keep two elements, got {items:?}"));
    };
    assert_float_literal(first, 4.0, &IrType::Float)?;
    assert!(
        matches!(second.kind, IrExprKind::Int(5)),
        "the int element stays an integer literal, got {second:?}"
    );

    let int_slot = written_value(stmts, "n", 0)?;
    assert!(
        matches!(int_slot.kind, IrExprKind::Int(6)),
        "an int slot keeps its integer literal, got {int_slot:?}"
    );
    let ratio = written_value(stmts, "ratio", 0)?;
    let IrExprKind::BinOp { left, right, .. } = &ratio.kind else {
        return Err(format!("`10 / 4` must lower to a division, got {ratio:?}"));
    };
    assert!(
        matches!((&left.kind, &right.kind), (IrExprKind::Int(10), IrExprKind::Int(4))),
        "the operands of `/` stay integer literals, got {left:?} and {right:?}"
    );
    Ok(())
}

/// #1847: a tuple literal is lowered with the element types of its annotation, so the emitter can type its `None`
/// and `Ok(...)` elements, down through a nested tuple.
#[test]
fn tuple_literal_keeps_its_annotated_element_types_issue1847() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def main() -> None:
    pair: tuple[Option[str], int] = (None, 1)
    ok_pair: tuple[Result[int, int], int] = (Ok(1), 0)
    nested: tuple[tuple[Option[int], int], int] = ((None, 1), 2)
    println(pair[1] + ok_pair[1] + nested[1])
"#,
    )?;
    let stmts = body(&ir, "main")?;

    let pair = written_value(stmts, "pair", 0)?;
    assert_eq!(
        pair.ty,
        IrType::Tuple(vec![IrType::Option(Box::new(IrType::String)), IrType::Int]),
        "the tuple literal carries its annotation's element types"
    );
    let ok_pair = written_value(stmts, "ok_pair", 0)?;
    assert_eq!(
        ok_pair.ty,
        IrType::Tuple(vec![
            IrType::Result(Box::new(IrType::Int), Box::new(IrType::Int)),
            IrType::Int
        ])
    );
    let nested = written_value(stmts, "nested", 0)?;
    let IrExprKind::Tuple(items) = &nested.kind else {
        return Err(format!(
            "the nested tuple literal must lower to a tuple, got {nested:?}"
        ));
    };
    let inner = items
        .first()
        .ok_or("the nested tuple literal must keep its inner tuple")?;
    assert_eq!(
        inner.ty,
        IrType::Tuple(vec![IrType::Option(Box::new(IrType::Int)), IrType::Int]),
        "the inner tuple literal carries the annotation's inner element types"
    );
    Ok(())
}

/// #1832: an empty or `None`-only list literal assigned to an existing `Option[list[...]]` binding is lowered with the
/// binding's list type, so the emitter wraps it in `Some` and types its `None` elements.
#[test]
fn list_literal_keeps_its_option_binding_type_issue1832() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def main() -> None:
    mut a: Option[list[int]] = None
    a = []
    mut b: Option[list[Option[str]]] = None
    b = [None]
    println(len(a.unwrap_or([])) + len(b.unwrap_or([])))
"#,
    )?;
    let stmts = body(&ir, "main")?;

    let empty = written_value(stmts, "a", 1)?;
    assert!(
        matches!(empty.kind, IrExprKind::List(_)),
        "`a = []` assigns the list literal itself, got {empty:?}"
    );
    assert_eq!(empty.ty, IrType::List(Box::new(IrType::Int)));
    let nones = written_value(stmts, "b", 1)?;
    assert_eq!(
        nones.ty,
        IrType::List(Box::new(IrType::Option(Box::new(IrType::String))))
    );
    Ok(())
}
