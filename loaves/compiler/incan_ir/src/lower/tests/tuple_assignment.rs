//! Tuple assignment into existing places: a tuple unpacking whose names are already bound reassigns them, and a tuple
//! assignment to fields and list elements writes each place. Both read the whole right side into one temporary first,
//! so a swap sees the values from before any write.

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

    let stmts = all_statements(&ir, "fib")?;
    let loop_body: Vec<&IrStmt> = stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::For { body, .. } => {
                let mut nested = Vec::new();
                flatten(body, &mut nested);
                Some(nested)
            }
            _ => None,
        })
        .ok_or_else(|| format!("`fib` must keep its loop: {stmts:?}"))?;
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
