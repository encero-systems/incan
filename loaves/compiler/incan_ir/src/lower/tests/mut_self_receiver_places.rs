//! A `mut self` call whose receiver reaches its value through a list element or a dict value borrows that receiver
//! mutably as a place, so the call changes the stored value rather than a copy read out of the collection (#1561).

use super::*;

/// For each method call in one function, in source order, whether its receiver is borrowed mutably as a place. The
/// method's lowered name is its declaration identity, so the calls are told apart by position.
#[derive(Default)]
struct CallReceivers(Vec<bool>);

impl crate::visit::Visitor for CallReceivers {
    fn expr(&mut self, expr: &mut crate::IrExpr) {
        if let IrExprKind::MethodCall { receiver, .. } = &expr.kind {
            self.0.push(matches!(
                receiver.kind,
                IrExprKind::UnaryOp {
                    op: UnaryOp::RefMut,
                    ..
                }
            ));
        }
        crate::visit::walk_expr(expr, self);
    }
}

/// `counters[0].bump()`, `table["a"].bump()` and `wraps[0].inner.bump()` borrow their receivers as places; `counter`
/// and the tuple element `pair[0]` are places already and keep their receivers as they are.
#[test]
fn mut_self_receiver_through_an_element_is_borrowed_as_a_place_issue1561() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
class Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1


class Wrap:
    pub inner: Counter


def main() -> None:
    mut counters = [Counter(count=0)]
    counters[0].bump()
    mut table: dict[str, Counter] = {"a": Counter(count=0)}
    table["a"].bump()
    mut wraps = [Wrap(inner=Counter(count=0))]
    wraps[0].inner.bump()
    mut counter = Counter(count=0)
    counter.bump()
    mut pair: tuple[Counter, int] = (Counter(count=0), 1)
    pair[0].bump()
"#,
    )?;
    let function = ir
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.kind {
            IrDeclKind::Function(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .ok_or("missing function `main`")?;
    let mut receivers = CallReceivers::default();
    for stmt in &mut function.body {
        crate::visit::Visitor::stmt(&mut receivers, stmt);
    }
    assert_eq!(receivers.0, vec![true, true, true, false, false]);
    Ok(())
}
