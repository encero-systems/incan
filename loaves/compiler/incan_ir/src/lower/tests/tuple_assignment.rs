//! Assignments with several targets into existing places: a tuple unpacking or a chained assignment whose names are
//! already bound reassigns them, and a tuple assignment to fields and list elements writes each place. The tuple
//! statements read the whole right side into one temporary first, so a swap sees the values from before any write.

use super::*;
use crate::stmt::AssignTarget;

/// Return the body of the named top-level function.
fn function_body<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [IrStmt], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function.body.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Collect every statement in `stmts` in source order, descending into loop bodies, branches and the
/// statement-position blocks a tuple statement lowers to.
fn flatten<'a>(stmts: &'a [IrStmt], out: &mut Vec<&'a IrStmt>) {
    for stmt in stmts {
        out.push(stmt);
        match &stmt.kind {
            IrStmtKind::Expr(expr) => {
                if let IrExprKind::Block { stmts: inner, .. } = &expr.kind {
                    flatten(inner, out);
                }
            }
            IrStmtKind::For { body, .. } | IrStmtKind::While { body, .. } | IrStmtKind::Loop { body, .. } => {
                flatten(body, out);
            }
            IrStmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                flatten(then_branch, out);
                if let Some(else_branch) = else_branch {
                    flatten(else_branch, out);
                }
            }
            IrStmtKind::Block(inner) => flatten(inner, out),
            _ => {}
        }
    }
}

/// Return every statement of the named function, nested statements included.
fn all_statements<'a>(ir: &'a IrProgram, name: &str) -> Result<Vec<&'a IrStmt>, String> {
    let mut out = Vec::new();
    flatten(function_body(ir, name)?, &mut out);
    Ok(out)
}

/// Return every statement of the first `for` loop in the named function, nested statements included.
fn loop_statements<'a>(ir: &'a IrProgram, name: &str) -> Result<Vec<&'a IrStmt>, String> {
    let stmts = all_statements(ir, name)?;
    stmts
        .iter()
        .copied()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::For { body, .. } => {
                let mut nested = Vec::new();
                flatten(body, &mut nested);
                Some(nested)
            }
            _ => None,
        })
        .ok_or_else(|| format!("`{name}` must keep its loop: {stmts:?}"))
}

/// Describe each assignment to a local name as `(name, source)`, where the source is the local the value reads, or
/// `None` for any other value.
fn name_assignments(stmts: &[&IrStmt]) -> Vec<(String, Option<String>)> {
    stmts
        .iter()
        .copied()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Var { name, .. },
                value,
            } => {
                let source = match &value.kind {
                    IrExprKind::Var { name, .. } => Some(name.clone()),
                    _ => None,
                };
                Some((name.clone(), source))
            }
            _ => None,
        })
        .collect()
}

/// Return whether `stmts` declare a local with the given name.
fn declares(stmts: &[&IrStmt], name: &str) -> bool {
    stmts
        .iter()
        .any(|stmt| matches!(&stmt.kind, IrStmtKind::Let { name: declared, .. } if declared == name))
}

/// Return the name of the temporary the tuple value is read into: the one `let` whose value is a tuple.
fn tuple_temporary(stmts: &[&IrStmt]) -> Result<(usize, String), String> {
    stmts
        .iter()
        .enumerate()
        .find_map(|(index, stmt)| match &stmt.kind {
            IrStmtKind::Let { name, value, .. } if matches!(value.kind, IrExprKind::Tuple(_)) => {
                Some((index, name.clone()))
            }
            _ => None,
        })
        .ok_or_else(|| format!("the tuple value must be read into a temporary first: {stmts:?}"))
}

/// Return the element index when `value` reads one element of the named temporary.
fn temporary_element(value: &TypedExpr, temporary: &str) -> Option<String> {
    match &value.kind {
        IrExprKind::Field { object, field } if matches!(&object.kind, IrExprKind::Var { name, .. } if name == temporary) => {
            Some(field.clone())
        }
        _ => None,
    }
}

/// Describe each assignment after the temporary as `(place, element)`, where the place is the assigned local name, the
/// assigned field name or `[]` for an element, and the element is the temporary's element it receives.
fn assignments_from_temporary(stmts: &[&IrStmt], after: usize, temporary: &str) -> Vec<(String, Option<String>)> {
    stmts[after + 1..]
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign { target, value } => {
                let place = match target {
                    AssignTarget::Var { name, .. } | AssignTarget::StaticBinding(name) => name.clone(),
                    AssignTarget::Static { name, .. } => name.clone(),
                    AssignTarget::Field { field, .. } => format!(".{field}"),
                    AssignTarget::Index { .. } => "[]".to_string(),
                };
                Some((place, temporary_element(value, temporary)))
            }
            _ => None,
        })
        .collect()
}

/// #1799: `a, b = (b, a + b)` inside a loop updates the loop's `a` and `b`. Lowering them as fresh `let` bindings shadowed
/// the outer names for one iteration only, so the loop never advanced.
#[test]
fn tuple_unpacking_into_bound_names_reassigns_them_inside_a_loop_issue1799() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def fib(n: int) -> int:
    mut a = 0
    mut b = 1
    for _ in range(n):
        a, b = (b, a + b)
    return a

def step(start: int) -> int:
    mut total = start
    total, extra = (total + 1, 5)
    return total + extra
"#,
    )?;

    let loop_body = loop_statements(&ir, "fib")?;
    assert!(
        !loop_body
            .iter()
            .any(|stmt| matches!(&stmt.kind, IrStmtKind::Let { name, .. } if name == "a" || name == "b")),
        "the loop must not bind fresh `a` or `b`: {loop_body:?}"
    );
    let (at, temporary) = tuple_temporary(&loop_body)?;
    assert_eq!(
        assignments_from_temporary(&loop_body, at, &temporary),
        vec![
            ("a".to_string(), Some("0".to_string())),
            ("b".to_string(), Some("1".to_string()))
        ],
        "each bound name is assigned its element of the temporary, in order"
    );

    // A new name beside a bound one is still declared; only the bound one is reassigned.
    let stmts = all_statements(&ir, "step")?;
    let (at, temporary) = tuple_temporary(&stmts)?;
    assert_eq!(
        assignments_from_temporary(&stmts, at, &temporary),
        vec![("total".to_string(), Some("0".to_string()))]
    );
    assert!(
        stmts[at + 1..]
            .iter()
            .any(|stmt| matches!(&stmt.kind, IrStmtKind::Let { name, .. } if name == "extra")),
        "`extra` is a new name and is declared: {stmts:?}"
    );
    Ok(())
}

/// #1798: `grid.width, grid.height = (grid.height, grid.width)` and `items[i], items[j] = (items[j], items[i])` write each
/// place from one temporary read before any write. Lowering refused both with "TupleAssign not yet implemented".
#[test]
fn tuple_assignment_to_fields_and_list_elements_writes_each_place_issue1798() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
class Grid:
    pub width: int
    pub height: int

def swap_fields(mut grid: Grid) -> Grid:
    grid.width, grid.height = (grid.height, grid.width)
    return grid

def swap_items(mut items: list[int], i: int, j: int) -> list[int]:
    items[i], items[j] = (items[j], items[i])
    return items

def trade(mut grid: Grid, start: int) -> int:
    mut total = start
    grid.width, total = (total, grid.width)
    return total + grid.width
"#,
    )?;

    for (name, expected) in [
        ("swap_fields", vec![".width", ".height"]),
        ("swap_items", vec!["[]", "[]"]),
        ("trade", vec![".width", "total"]),
    ] {
        let stmts = all_statements(&ir, name)?;
        let (at, temporary) = tuple_temporary(&stmts)?;
        let assignments = assignments_from_temporary(&stmts, at, &temporary);
        assert_eq!(
            assignments,
            expected
                .iter()
                .zip(["0", "1"])
                .map(|(place, element)| (place.to_string(), Some(element.to_string())))
                .collect::<Vec<_>>(),
            "`{name}` writes each place from its element of the temporary, in order"
        );
    }
    Ok(())
}

/// Describe each assignment and each declaration among `stmts` as `(name, access)` when its value reads the chain's
/// value temporary, in order.
fn chain_value_reads(stmts: &[&IrStmt]) -> Vec<(String, VarAccess)> {
    stmts
        .iter()
        .copied()
        .filter_map(|stmt| {
            let (name, value) = match &stmt.kind {
                IrStmtKind::Assign {
                    target: AssignTarget::Var { name, .. },
                    value,
                } => (name, value),
                IrStmtKind::Let { name, value, .. } => (name, value),
                _ => return None,
            };
            match &value.kind {
                IrExprKind::Var {
                    name: source, access, ..
                } if source == CHAIN_VALUE => Some((name.clone(), *access)),
                _ => None,
            }
        })
        .collect()
}

/// The temporary a chained assignment reads its value into.
const CHAIN_VALUE: &str = "__incan_chain_value";

/// #1806: `x = y = x + 1` inside a loop updates the loop's `x`, and `y`, first bound by the chain, is declared. Lowering
/// declared every target with a fresh `let`, so `x` was shadowed for one iteration only. Every target takes the value
/// from one temporary, left to right.
#[test]
fn chained_assignment_into_bound_names_reassigns_them_inside_a_loop_issue1806() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def count_up(n: int) -> int:
    mut x = 0
    for _ in range(n):
        x = y = x + 1
    return x

def both(n: int) -> int:
    mut total = 0
    mut last = 0
    for _ in range(n):
        total = last = total + 2
    return total + last
"#,
    )?;

    let body = loop_statements(&ir, "count_up")?;
    assert!(!declares(&body, "x"), "the loop must not bind a fresh `x`: {body:?}");
    assert_eq!(
        chain_value_reads(&body),
        vec![("x".to_string(), VarAccess::Copy), ("y".to_string(), VarAccess::Copy)],
        "`x` is assigned and `y` declared from the chain's value, left to right"
    );
    assert!(
        declares(&body, "y"),
        "`y` is first bound by the chain and is declared: {body:?}"
    );

    let body = loop_statements(&ir, "both")?;
    assert!(
        !declares(&body, "total") && !declares(&body, "last"),
        "the loop must not bind a fresh `total` or `last`: {body:?}"
    );
    assert_eq!(
        name_assignments(&body),
        vec![
            ("total".to_string(), Some(CHAIN_VALUE.to_string())),
            ("last".to_string(), Some(CHAIN_VALUE.to_string()))
        ],
        "every target takes the chain's value, left to right"
    );
    Ok(())
}

/// #1806: a target never reads another target. `count = maybe = seed` gives `seed` to the `int` and to the
/// `Option[int]`, and `n = u = seed` to the `int` and to the `int | str`, each converted by its own target; a value
/// that is not `Copy` is cloned for every target but the last, which takes the temporary itself.
#[test]
fn chained_assignment_gives_every_target_the_value_issue1806() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def option_target(seed: int) -> int:
    mut maybe: Option[int] = None
    mut count = 0
    count = maybe = seed
    return count

def union_target(seed: int) -> int:
    mut u: int | str = "s"
    mut n = 0
    n = u = seed
    return n

def strings() -> str:
    mut a = "x"
    mut b = "y"
    a = b = "z"
    return a + b

def lists() -> int:
    x = y = z = [1]
    return len(x) + len(y) + len(z)

def new_names() -> str:
    first = second = "x"
    return first + second
"#,
    )?;

    for (name, targets) in [("option_target", ["count", "maybe"]), ("union_target", ["n", "u"])] {
        let stmts = all_statements(&ir, name)?;
        assert_eq!(
            chain_value_reads(&stmts),
            targets
                .iter()
                .map(|target| (target.to_string(), VarAccess::Copy))
                .collect::<Vec<_>>(),
            "`{name}` gives the value to each target, left to right"
        );
    }
    for (name, targets) in [
        ("strings", vec!["a", "b"]),
        ("lists", vec!["x", "y", "z"]),
        ("new_names", vec!["first", "second"]),
    ] {
        let stmts = all_statements(&ir, name)?;
        let last = targets.len() - 1;
        assert_eq!(
            chain_value_reads(&stmts),
            targets
                .iter()
                .enumerate()
                .map(|(index, target)| {
                    let access = if index == last {
                        VarAccess::Move
                    } else {
                        VarAccess::Read
                    };
                    (target.to_string(), access)
                })
                .collect::<Vec<_>>(),
            "`{name}` clones the value for every target but the last"
        );
    }
    Ok(())
}

/// Return the declared type and annotation of the chain's value temporary among `stmts`.
fn chain_value_type<'a>(stmts: &[&'a IrStmt]) -> Result<(&'a IrType, Option<&'a IrType>), String> {
    stmts
        .iter()
        .copied()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Let {
                name,
                ty,
                type_annotation,
                ..
            } if name == CHAIN_VALUE => Some((ty, type_annotation.as_ref())),
            _ => None,
        })
        .ok_or_else(|| format!("the chain must read its value into `{CHAIN_VALUE}`: {stmts:?}"))
}

/// #1806: the chain's temporary has the type its bound targets agree on, so `a = b = None` over two `Option[int]`
/// targets is an `Option[int]` rather than an `Option` of nothing; when the targets disagree it keeps the value's own
/// type. A module static before the last target takes an explicit copy, since its assignment takes what it is given.
#[test]
fn chained_assignment_types_its_value_by_the_targets_and_copies_for_a_static_issue1806() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
static names: list[str] = []

def reset() -> int:
    mut a: Option[int] = Some(1)
    mut b: Option[int] = Some(2)
    a = b = None
    return a.unwrap_or(0) + b.unwrap_or(0)

def mixed(seed: int) -> int:
    mut maybe: Option[int] = None
    mut count = 0
    count = maybe = seed
    return count

def fill() -> int:
    names = local = ["a"]
    return len(local)
"#,
    )?;

    let stmts = all_statements(&ir, "reset")?;
    let option_int = IrType::Option(Box::new(IrType::Int));
    assert_eq!(
        chain_value_type(&stmts)?,
        (&option_int, Some(&option_int)),
        "`a = b = None` reads `None` as the targets' `Option[int]`"
    );

    let stmts = all_statements(&ir, "mixed")?;
    assert_eq!(
        chain_value_type(&stmts)?,
        (&IrType::Int, None),
        "targets that disagree leave the value its own type"
    );

    let stmts = all_statements(&ir, "fill")?;
    let static_value = stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign {
                target: AssignTarget::Static { name, .. } | AssignTarget::StaticBinding(name),
                value,
            } if name == "names" => Some(value),
            _ => None,
        })
        .ok_or_else(|| format!("`names` must be assigned: {stmts:?}"))?;
    assert!(
        matches!(&static_value.kind, IrExprKind::MethodCall { method, .. } if method == "clone"),
        "a static before the last target takes an explicit copy, got {static_value:?}"
    );
    assert_eq!(
        chain_value_reads(&stmts),
        vec![("local".to_string(), VarAccess::Move)],
        "the last target takes the value itself"
    );
    Ok(())
}

/// #1806: a value built only from literals over bound targets that disagree on a type is written once per target, left
/// to right, as a single `target = value` writes it: `None`, `(None)`, `[None]`, `[]`, `{}` and `list()` over targets
/// of different types have no one type to share, so no temporary is read, and each copy takes its target's type.
#[test]
fn chained_literal_over_disagreeing_targets_is_written_per_target_issue1806() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def options() -> None:
    mut a: Option[int] = Some(1)
    mut b: Option[str] = Some("s")
    a = b = None

def lists() -> None:
    mut ints: list[int] = [1]
    mut strs: list[str] = ["s"]
    ints = strs = []

def dicts() -> None:
    mut by_name: dict[str, int] = {"a": 1}
    mut by_id: dict[int, str] = {1: "a"}
    by_name = by_id = {}

def numbers() -> None:
    mut maybe: Option[int] = None
    mut count = 0
    count = maybe = 5

def parenthesized() -> None:
    mut a: Option[int] = Some(1)
    mut b: Option[str] = Some("s")
    a = b = (None)

def nested() -> None:
    mut xs: list[Option[int]] = []
    mut ys: list[Option[str]] = []
    xs = ys = [None]

def constructed() -> None:
    mut ints: list[int] = [1]
    mut strs: list[str] = ["s"]
    ints = strs = list()
"#,
    )?;

    for (name, targets) in [
        ("options", ["a", "b"]),
        ("lists", ["ints", "strs"]),
        ("dicts", ["by_name", "by_id"]),
        ("numbers", ["count", "maybe"]),
        ("parenthesized", ["a", "b"]),
        ("nested", ["xs", "ys"]),
        ("constructed", ["ints", "strs"]),
    ] {
        let stmts = all_statements(&ir, name)?;
        assert!(
            !stmts
                .iter()
                .any(|stmt| matches!(&stmt.kind, IrStmtKind::Let { name, .. } if name == CHAIN_VALUE)),
            "`{name}` has no one type to share, so it reads no temporary: {stmts:?}"
        );
        let assigned = stmts
            .iter()
            .filter_map(|stmt| match &stmt.kind {
                IrStmtKind::Assign {
                    target: AssignTarget::Var { name, ty },
                    value,
                } => Some((name.clone(), ty.clone(), value.ty.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            assigned
                .iter()
                .map(|(target, _, _)| target.as_str())
                .collect::<Vec<_>>(),
            targets,
            "`{name}` writes each target, left to right"
        );
        if name != "numbers" {
            for (target, target_ty, value_ty) in &assigned {
                assert_eq!(value_ty, target_ty, "`{target}` gets the literal in its own type");
            }
        }
    }

    // Each copy of `[None]` gives its `None` its own list's element type.
    let stmts = all_statements(&ir, "nested")?;
    let element_types = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Assign { value, .. } => match &value.kind {
                IrExprKind::List(entries) => entries.iter().find_map(|entry| match entry {
                    crate::expr::IrListEntry::Element(item) => Some(item.ty.clone()),
                    crate::expr::IrListEntry::Spread(_) => None,
                }),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        element_types,
        vec![
            IrType::Option(Box::new(IrType::Int)),
            IrType::Option(Box::new(IrType::String))
        ],
        "each list's `None` takes that list's element type"
    );
    Ok(())
}
