//! Lowering keeps the type the checker gave a literal from its destination: an integer literal in a float slot is
//! lowered as a float literal, and each target's copy of a chained integer literal takes that target's type (#1831),
//! a tuple literal carries the destination's element types (#1847), and a list literal assigned to an
//! `Option[list[...]]` binding carries the list type (#1832). A value written to an `Option` field, element or return
//! type (#1858), or to a nested `Option` binding (#1860), lowers to a `Some` call per layer, and the fallback of
//! `Option[str].unwrap_or` takes Incan value semantics (#1875).

use super::*;
use crate::stmt::AssignTarget;
use incan_lang::lang::surface::constructors::{self, ConstructorId};

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

/// #1847: a tuple or list literal declared in a generic body takes its annotation's type, so its `None` elements are
/// typed with the body's own type parameter rather than one still to be inferred; a literal whose type mentions no
/// type parameter keeps its own type.
#[test]
fn literal_in_a_generic_body_takes_its_annotated_type_issue1847() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def mk[T](x: T) -> int:
    pair: tuple[T, Option[T]] = (x, None)
    items: list[Option[T]] = [None]
    count: tuple[int, int] = (1, 2)
    return len(items) + count[0]


def main() -> None:
    println(mk("a"))
"#,
    )?;
    let stmts = body(&ir, "mk")?;
    for name in ["pair", "items"] {
        let Some((declared, value)) = stmts.iter().find_map(|stmt| match &stmt.kind {
            IrStmtKind::Let {
                name: bound,
                type_annotation: Some(declared),
                value,
                ..
            } if bound == name => Some((declared, value)),
            _ => None,
        }) else {
            return Err(format!("missing annotated declaration of `{name}`"));
        };
        assert_eq!(&value.ty, declared, "`{name}`'s literal takes its annotation's type");
    }
    let count = written_value(stmts, "count", 0)?;
    assert_eq!(count.ty, IrType::Tuple(vec![IrType::Int, IrType::Int]));
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

/// Return the `(target, value)` pairs the chained assignments in `stmts` write or declare, in the order they write
/// them.
fn chained_writes(stmts: &[IrStmt]) -> Vec<(&str, &TypedExpr)> {
    stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Expr(expr) => match &expr.kind {
                IrExprKind::Block { stmts: inner, .. } => Some(inner),
                _ => None,
            },
            _ => None,
        })
        .flatten()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Var { name, .. },
                value,
            }
            | IrStmtKind::Let { name, value, .. } => Some((name.as_str(), value)),
            _ => None,
        })
        .collect()
}

/// Assert that `expr` is the integer literal `magnitude` in the scalar type `ty`, negated when `negated`: an integer
/// literal for `int`, a float literal for a float type.
fn assert_literal_in_type(expr: &TypedExpr, magnitude: i64, negated: bool, ty: &IrType) -> Result<(), String> {
    if &expr.ty != ty {
        return Err(format!("expected a value of type {ty:?}, got {expr:?}"));
    }
    let literal = if negated {
        let IrExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } = &expr.kind
        else {
            return Err(format!("expected a negation, got {expr:?}"));
        };
        operand.as_ref()
    } else {
        expr
    };
    if *ty == IrType::Int {
        return match literal.kind {
            IrExprKind::Int(value) if value == magnitude && literal.ty == IrType::Int => Ok(()),
            _ => Err(format!("expected the integer literal {magnitude}, got {literal:?}")),
        };
    }
    assert_float_literal(literal, magnitude as f64, ty)
}

/// #1831: a chained assignment of an integer literal, or of its negation, over an integer target and a float target
/// writes the literal in each target's own type, whichever target comes last: the checker records one type for the
/// value's source span, the last target's, and each target's copy must still be of its own type.
#[test]
fn chained_integer_literal_takes_each_targets_type_issue1831() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def int_first() -> None:
    mut n: int = 0
    mut f: float = 0.5
    n = f = 1
    n = f = -1
    println(n)
    println(f)

def float_first() -> None:
    mut n: int = 0
    mut f: float = 0.5
    f = n = 1
    f = n = -1
    println(n)
    println(f)

def narrow_float() -> None:
    mut n: int = 0
    mut g: f32 = 0.5
    n = g = 1
    g = n = -1
    println(n)
    println(g)
"#,
    )?;
    let f32_ty = IrType::Numeric(NumericTypeId::F32);
    for (function, expected) in [
        (
            "int_first",
            [
                ("n", false, IrType::Int),
                ("f", false, IrType::Float),
                ("n", true, IrType::Int),
                ("f", true, IrType::Float),
            ],
        ),
        (
            "float_first",
            [
                ("f", false, IrType::Float),
                ("n", false, IrType::Int),
                ("f", true, IrType::Float),
                ("n", true, IrType::Int),
            ],
        ),
        (
            "narrow_float",
            [
                ("n", false, IrType::Int),
                ("g", false, f32_ty.clone()),
                ("g", true, f32_ty.clone()),
                ("n", true, IrType::Int),
            ],
        ),
    ] {
        let writes = chained_writes(body(&ir, function)?);
        assert_eq!(
            writes.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
            expected.iter().map(|(target, _, _)| *target).collect::<Vec<_>>(),
            "`{function}` writes each chain's targets, left to right"
        );
        for ((target, value), (_, negated, ty)) in writes.iter().zip(&expected) {
            assert_literal_in_type(value, 1, *negated, ty)
                .map_err(|message| format!("`{function}`, target `{target}`: {message}"))?;
        }
    }
    Ok(())
}

/// Return the elements of a lowered list literal.
fn literal_elements(expr: &TypedExpr) -> Result<Vec<&TypedExpr>, String> {
    match &expr.kind {
        IrExprKind::List(entries) => entries
            .iter()
            .map(|entry| match entry {
                crate::expr::IrListEntry::Element(item) => Ok(item),
                crate::expr::IrListEntry::Spread(_) => Err(format!("unexpected spread in {expr:?}")),
            })
            .collect(),
        _ => Err(format!("expected a list literal, got {expr:?}")),
    }
}

/// #1831: each target's copy of a chained value takes its own type at every integer literal inside it too: the
/// elements of a list or dict literal, a parenthesized literal, and the copy for a name the chain declares,
/// which is an `int` as a declaration without a destination makes it.
#[test]
fn chained_integer_literal_elements_take_each_targets_type_issue1831() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def lists() -> None:
    mut ints: list[int] = []
    mut floats: list[float] = []
    ints = floats = [1, -1]
    floats = ints = [1, -1]
    println(len(ints) + len(floats))

def dicts() -> None:
    mut counts: dict[str, int] = {}
    mut weights: dict[str, float] = {}
    counts = weights = {"a": 1}
    println(len(counts) + len(weights))

def parenthesized() -> None:
    mut n: int = 0
    mut f: float = 0.5
    n = f = (1)
    println(n)
    println(f)

def declared() -> None:
    mut n: int = 0
    mut f: float = 0.5
    x = n = f = 1
    println(x + n)
    println(f)
"#,
    )?;
    let float_list = [(false, IrType::Float), (true, IrType::Float)];
    let int_list = [(false, IrType::Int), (true, IrType::Int)];
    let writes = chained_writes(body(&ir, "lists")?);
    let expected_lists = [
        ("ints", &int_list),
        ("floats", &float_list),
        ("floats", &float_list),
        ("ints", &int_list),
    ];
    assert_eq!(
        writes.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
        expected_lists.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
        "`lists` writes each chain's targets, left to right"
    );
    for ((target, value), (_, elements)) in writes.iter().zip(expected_lists) {
        for (element, (negated, ty)) in literal_elements(value)?.into_iter().zip(elements) {
            assert_literal_in_type(element, 1, *negated, ty)
                .map_err(|message| format!("`lists`, target `{target}`: {message}"))?;
        }
    }

    let writes = chained_writes(body(&ir, "dicts")?);
    let expected_dicts = [("counts", IrType::Int), ("weights", IrType::Float)];
    assert_eq!(
        writes.len(),
        expected_dicts.len(),
        "`dicts` writes both targets: {writes:?}"
    );
    for ((target, value), (_, ty)) in writes.iter().zip(&expected_dicts) {
        let IrExprKind::Dict(entries) = &value.kind else {
            return Err(format!(
                "`dicts`, target `{target}`: expected a dict literal, got {value:?}"
            ));
        };
        for entry in entries {
            let crate::expr::IrDictEntry::Pair(_, item) = entry else {
                return Err(format!("`dicts`, target `{target}`: unexpected spread in {value:?}"));
            };
            assert_literal_in_type(item, 1, false, ty)
                .map_err(|message| format!("`dicts`, target `{target}`: {message}"))?;
        }
    }

    for (function, expected) in [
        ("parenthesized", vec![("n", IrType::Int), ("f", IrType::Float)]),
        (
            "declared",
            vec![("x", IrType::Int), ("n", IrType::Int), ("f", IrType::Float)],
        ),
    ] {
        let writes = chained_writes(body(&ir, function)?);
        assert_eq!(
            writes.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
            expected.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
            "`{function}` writes each target, left to right"
        );
        for ((target, value), (_, ty)) in writes.iter().zip(&expected) {
            assert_literal_in_type(value, 1, false, ty)
                .map_err(|message| format!("`{function}`, target `{target}`: {message}"))?;
        }
    }
    Ok(())
}

/// Return the payload of `expr` when it is a `Some(...)` call of type `option_ty`.
fn some_payload<'ir>(expr: &'ir TypedExpr, option_ty: &IrType) -> Result<&'ir TypedExpr, String> {
    let IrExprKind::Call { func, args, .. } = &expr.kind else {
        return Err(format!("expected a `Some` call of type {option_ty:?}, got {expr:?}"));
    };
    let calls_some = matches!(
        &func.kind,
        IrExprKind::Var { name, .. } if name == constructors::as_str(ConstructorId::Some)
    );
    match (calls_some, args.as_slice()) {
        (true, [payload]) if &expr.ty == option_ty => Ok(&payload.expr),
        _ => Err(format!("expected a `Some` call of type {option_ty:?}, got {expr:?}")),
    }
}

/// Return the value the `occurrence`-th assignment to the field `field` writes.
fn field_write<'ir>(stmts: &'ir [IrStmt], field: &str, occurrence: usize) -> Result<&'ir TypedExpr, String> {
    stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Field { field: written, .. },
                value,
            } if written == field => Some(value),
            _ => None,
        })
        .nth(occurrence)
        .ok_or_else(|| format!("missing write {occurrence} of field `{field}`"))
}

/// #1858: a value of an `Option`'s payload type written to an `Option` return type, field or list element lowers to a
/// `Some` call of the place's type around the value; a value already of the `Option` type is lowered as it is.
#[test]
fn value_written_to_an_option_place_lowers_to_some_issue1858() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
model Box:
    count: Option[int] = None


def make() -> Option[int]:
    return 5


def main() -> None:
    mut box = Box()
    box.count = 6
    box.count = Some(7)
    mut slots: list[Option[int]] = [None]
    slots[0] = 8
    println(make().unwrap_or(0) + slots[0].unwrap_or(0))
"#,
    )?;
    let option_int = IrType::Option(Box::new(IrType::Int));
    let returned = body(&ir, "make")?
        .iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Return(Some(value)) => Some(value),
            _ => None,
        })
        .ok_or("missing the return of `make`")?;
    let payload = some_payload(returned, &option_int)?;
    assert!(matches!(payload.kind, IrExprKind::Int(5)), "got {payload:?}");

    let stmts = body(&ir, "main")?;
    let payload = some_payload(field_write(stmts, "count", 0)?, &option_int)?;
    assert!(matches!(payload.kind, IrExprKind::Int(6)), "got {payload:?}");
    let payload = some_payload(field_write(stmts, "count", 1)?, &option_int)?;
    assert!(
        matches!(payload.kind, IrExprKind::Int(7)),
        "`Some(7)` is written as it is, got {payload:?}"
    );
    let element = stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Index { .. },
                value,
            } => Some(value),
            _ => None,
        })
        .ok_or("missing the element write")?;
    let payload = some_payload(element, &option_int)?;
    assert!(matches!(payload.kind, IrExprKind::Int(8)), "got {payload:?}");
    Ok(())
}

/// #1860: a value assigned to a binding of a nested `Option` type lowers to one `Some` call per layer, in a
/// declaration and a reassignment; a value one layer short of its binding's type is lowered as it is.
#[test]
fn value_assigned_to_a_nested_option_binding_lowers_to_nested_some_issue1860() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def main() -> None:
    a: Option[Option[int]] = 5
    mut b: Option[Option[list[int]]] = None
    b = []
    one: Option[int] = 6
    println(a.unwrap().unwrap() + one.unwrap_or(0) + len(b.unwrap().unwrap_or([])))
"#,
    )?;
    let stmts = body(&ir, "main")?;
    let option_int = IrType::Option(Box::new(IrType::Int));
    let outer = some_payload(
        written_value(stmts, "a", 0)?,
        &IrType::Option(Box::new(option_int.clone())),
    )?;
    let payload = some_payload(outer, &option_int)?;
    assert!(matches!(payload.kind, IrExprKind::Int(5)), "got {payload:?}");

    let int_list = IrType::List(Box::new(IrType::Int));
    let option_list = IrType::Option(Box::new(int_list.clone()));
    let outer = some_payload(
        written_value(stmts, "b", 1)?,
        &IrType::Option(Box::new(option_list.clone())),
    )?;
    let payload = some_payload(outer, &option_list)?;
    assert!(
        matches!(payload.kind, IrExprKind::List(_)) && payload.ty == int_list,
        "got {payload:?}"
    );

    let one = written_value(stmts, "one", 0)?;
    assert!(
        matches!(one.kind, IrExprKind::Int(6)),
        "a value one layer short is lowered as it is, got {one:?}"
    );
    Ok(())
}

/// #1875: `unwrap_or` on an `Option[str]` takes its fallback with Incan value semantics, so a `str` binding is passed
/// as the owned `str` the call returns; on an `Option[int]` the argument keeps the default policy.
#[test]
fn option_str_unwrap_or_takes_its_fallback_owned_issue1875() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def main() -> None:
    d: Option[str] = None
    missing = "none"
    text = d.unwrap_or(missing)
    n: Option[int] = None
    fallback = 3
    number = n.unwrap_or(fallback)
    println(text)
    println(number)
"#,
    )?;
    let stmts = body(&ir, "main")?;
    for (name, expected) in [
        ("text", MethodCallArgPolicy::SourceOwned),
        ("number", MethodCallArgPolicy::Default),
    ] {
        let value = written_value(stmts, name, 0)?;
        let IrExprKind::MethodCall { method, arg_policy, .. } = &value.kind else {
            return Err(format!("`{name}` must be a method call, got {value:?}"));
        };
        assert_eq!(method, "unwrap_or");
        assert_eq!(*arg_policy, expected, "the argument policy of `{name}`'s `unwrap_or`");
    }
    Ok(())
}
