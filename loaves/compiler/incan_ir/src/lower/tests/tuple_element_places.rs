//! A constant tuple index lowers to the element's place, a field named by the element's position, also when the tuple
//! is a `mut` parameter and so a reference to the caller's tuple (#1561).

use super::*;
use crate::expr::positional_field_index;

/// How each index of the local `pair` in one function lowered, in source order: `Some(position)` for a tuple element
/// place, `None` for a collection index.
#[derive(Default)]
struct PairIndexes(Vec<Option<usize>>);

impl crate::visit::Visitor for PairIndexes {
    fn expr(&mut self, expr: &mut crate::IrExpr) {
        match &expr.kind {
            IrExprKind::Field { object, field } if reads_pair(object) => {
                self.0.push(positional_field_index(field));
            }
            IrExprKind::Index { object, .. } if reads_pair(object) => self.0.push(None),
            _ => {}
        }
        crate::visit::walk_expr(expr, self);
    }
}

/// Whether `expr` reads the local `pair`.
fn reads_pair(expr: &TypedExpr) -> bool {
    matches!(&expr.kind, IrExprKind::Var { name, .. } if name == "pair")
}

/// Collect how `pair` is indexed in the function named `name` of a checked, lowered program.
fn pair_indexes(ir: &mut IrProgram, name: &str) -> Result<Vec<Option<usize>>, String> {
    let function = ir
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))?;
    let mut indexes = PairIndexes::default();
    for stmt in &mut function.body {
        crate::visit::Visitor::stmt(&mut indexes, stmt);
    }
    Ok(indexes.0)
}

/// `pair[0]`, `pair[-2]` and `pair[1]` of a `mut` tuple parameter lower to the places of elements 0, 0 and 1, not to a
/// collection index into the reference the parameter is.
#[test]
fn tuple_index_through_a_mut_parameter_is_an_element_place_issue1561() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
def extend(mut pair: tuple[list[int], int], value: int) -> int:
    pair[0].append(value)
    return len(pair[-2]) + pair[1]
"#,
    )?;
    assert_eq!(pair_indexes(&mut ir, "extend")?, vec![Some(0), Some(0), Some(1)]);
    Ok(())
}
