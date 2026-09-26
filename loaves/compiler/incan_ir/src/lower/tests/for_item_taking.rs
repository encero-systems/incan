//! A `for` loop that takes the items of its list (#1844): the loop iterates the list by value, and the owned item's
//! final read in the body moves it. Loops over copyable or cloneable items keep iterating the list in place.

use super::*;

/// The iterables of every `for` loop and the operands of every `await` in the named function, in source order.
#[derive(Default)]
struct LoopShapes {
    iterables: Vec<TypedExpr>,
    awaited: Vec<TypedExpr>,
    call_arguments: Vec<TypedExpr>,
}

impl crate::visit::Visitor for LoopShapes {
    fn stmt(&mut self, stmt: &mut crate::IrStmt) {
        if let IrStmtKind::For { iterable, .. } = &stmt.kind {
            self.iterables.push(iterable.clone());
        }
        crate::visit::walk_stmt(stmt, self);
    }

    fn expr(&mut self, expr: &mut crate::IrExpr) {
        match &expr.kind {
            IrExprKind::Await(operand) => self.awaited.push(operand.as_ref().clone()),
            IrExprKind::Call { args, .. } => self.call_arguments.extend(args.iter().map(|arg| arg.expr.clone())),
            _ => {}
        }
        crate::visit::walk_expr(expr, self);
    }
}

/// Collect the loop shapes of the function named `name` in a checked, lowered program.
fn loop_shapes(ir: &mut IrProgram, name: &str) -> Result<LoopShapes, String> {
    let function = ir
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))?;
    let mut shapes = LoopShapes::default();
    for stmt in &mut function.body {
        crate::visit::Visitor::stmt(&mut shapes, stmt);
    }
    Ok(shapes)
}

/// Return the variable an expression reads and how, when it is one.
fn var_read(expr: &TypedExpr) -> Option<(&str, VarAccess)> {
    match &expr.kind {
        IrExprKind::Var { name, access, .. } => Some((name.as_str(), *access)),
        _ => None,
    }
}

/// #1844: awaiting each task handle of a list takes the handles out of the list. The loop iterates the list by value
/// through `into_iter()`, moving the list into the loop, and awaits each owned handle.
#[test]
fn loop_awaiting_each_handle_iterates_the_list_by_value_issue1844() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
from std.async import spawn

async def work() -> int:
    return 1

async def main() -> None:
    handles = [spawn(work()), spawn(work())]
    for handle in handles:
        match await handle:
            Ok(value) => println(value)
            Err(_) => println("join failed")
"#,
    )?;
    let shapes = loop_shapes(&mut ir, "main")?;
    let [iterable] = shapes.iterables.as_slice() else {
        return Err(format!("expected one loop, got {:?}", shapes.iterables));
    };
    let IrExprKind::MethodCall {
        receiver, method, args, ..
    } = &iterable.kind
    else {
        return Err(format!("the loop must iterate the list by value, got {iterable:?}"));
    };
    assert_eq!(method, "into_iter");
    assert!(args.is_empty(), "{args:?}");
    assert_eq!(var_read(receiver), Some(("handles", VarAccess::Move)), "{receiver:?}");
    assert!(
        matches!(iterable.ty, IrType::List(_)),
        "the loop item type still comes from the list: {:?}",
        iterable.ty
    );
    let [awaited] = shapes.awaited.as_slice() else {
        return Err(format!("expected one await, got {:?}", shapes.awaited));
    };
    assert_eq!(var_read(awaited), Some(("handle", VarAccess::Move)), "{awaited:?}");
    Ok(())
}

/// #1844: a handle passed to a call is moved into it at its final read in the loop body, whatever a later binding of
/// the same spelling outside the loop does; a read before the final one stays non-consuming.
#[test]
fn handle_passed_on_moves_at_its_final_read_in_the_loop_body_issue1844() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
from std.async import spawn, JoinHandle

async def work() -> int:
    return 1

async def wait_one(handle: JoinHandle[int]) -> int:
    match await handle:
        Ok(value) => return value
        Err(_) => return 0

async def main() -> None:
    handles = [spawn(work()), spawn(work())]
    mut total = 0
    for handle in handles:
        total += await wait_one(handle)
    println(total)
    handle = spawn(work())
    println(await wait_one(handle))
"#,
    )?;
    let shapes = loop_shapes(&mut ir, "main")?;
    let [iterable] = shapes.iterables.as_slice() else {
        return Err(format!("expected one loop, got {:?}", shapes.iterables));
    };
    assert!(
        matches!(&iterable.kind, IrExprKind::MethodCall { method, .. } if method == "into_iter"),
        "the loop must iterate the list by value, got {iterable:?}"
    );
    let passed: Vec<_> = shapes.call_arguments.iter().filter_map(var_read).collect();
    assert_eq!(
        passed,
        [("handle", VarAccess::Move), ("handle", VarAccess::Move)],
        "the loop item and the later handle both move into `wait_one`"
    );
    Ok(())
}

/// #1844: copyable and cloneable items stay in their list. A loop that reads, passes on or returns them keeps
/// iterating the list in place, and so does a loop over handles that does not hand the handle on.
#[test]
fn copyable_and_cloneable_items_keep_their_list_issue1844() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
from std.async import spawn

model Item:
    name: str

async def work() -> int:
    return 1

def shout(text: str) -> str:
    return text.upper()

def describe(item: Item) -> str:
    return item.name

def first_long(names: list[str]) -> str:
    for name in names:
        if len(name) > 3:
            return name
    return ""

async def main() -> None:
    names = ["ada", "grace"]
    for name in names:
        println(shout(name))
    items = [Item(name="a")]
    for item in items:
        println(describe(item))
    values = [1, 2]
    for value in values:
        println(value)
    handles = [spawn(work())]
    for handle in handles:
        pass
    println(len(names) + len(items) + len(values) + len(handles))
    println(first_long(names))
"#,
    )?;
    for function in ["main", "first_long"] {
        let shapes = loop_shapes(&mut ir, function)?;
        assert!(!shapes.iterables.is_empty(), "`{function}` must have loops");
        for iterable in &shapes.iterables {
            assert!(
                matches!(&iterable.kind, IrExprKind::Var { .. }),
                "a loop in `{function}` must keep iterating its list in place, got {iterable:?}"
            );
        }
    }
    Ok(())
}

/// #1844: channel ends and locks can be cloned at runtime, so loops that pass each one on keep iterating their list in
/// place and the generated Rust stays what it was; a parenthesized list of handles is taken like the bare name.
#[test]
fn channel_ends_and_locks_keep_their_list_issue1844() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
from std.async import channel, spawn, Sender, Mutex

async def work() -> int:
    return 1

def forward(tx: Sender[int]) -> None:
    pass

def hold(lock: Mutex[int]) -> None:
    pass

async def main() -> None:
    tx, rx = channel(4)
    senders: list[Sender[int]] = [tx]
    for sender in senders:
        forward(sender)
    locks: list[Mutex[int]] = [Mutex.new(1)]
    for lock in locks:
        hold(lock)
    println(len(senders) + len(locks))
    handles = [spawn(work())]
    for handle in (handles):
        match await handle:
            Ok(value) => println(value)
            Err(_) => println("join failed")
"#,
    )?;
    let shapes = loop_shapes(&mut ir, "main")?;
    let [senders, locks, handles] = shapes.iterables.as_slice() else {
        return Err(format!("expected three loops, got {:?}", shapes.iterables));
    };
    for (iterable, list) in [(senders, "senders"), (locks, "locks")] {
        assert!(
            matches!(&iterable.kind, IrExprKind::Var { name, .. } if name == list),
            "the loop over `{list}` must keep iterating the list in place, got {iterable:?}"
        );
    }
    assert!(
        matches!(&handles.kind, IrExprKind::MethodCall { method, .. } if method == "into_iter"),
        "the loop over the parenthesized handles must iterate the list by value, got {handles:?}"
    );
    Ok(())
}

/// #1844: a loop over the binding of an enclosing loop that takes its items iterates that binding by value, so each
/// handle of a list of lists is awaited owned.
#[test]
fn nested_loop_over_an_owned_loop_binding_iterates_it_by_value_issue1844() -> Result<(), String> {
    let mut ir = lower_checked_source(
        r#"
from std.async import spawn

async def work() -> int:
    return 1

async def main() -> None:
    groups = [[spawn(work())], [spawn(work()), spawn(work())]]
    for group in groups:
        for handle in group:
            match await handle:
                Ok(value) => println(value)
                Err(_) => println("join failed")
"#,
    )?;
    let shapes = loop_shapes(&mut ir, "main")?;
    let [outer, inner] = shapes.iterables.as_slice() else {
        return Err(format!("expected two loops, got {:?}", shapes.iterables));
    };
    for (iterable, list) in [(outer, "groups"), (inner, "group")] {
        let IrExprKind::MethodCall { receiver, method, .. } = &iterable.kind else {
            return Err(format!(
                "the loop over `{list}` must iterate it by value, got {iterable:?}"
            ));
        };
        assert_eq!(method, "into_iter");
        assert_eq!(var_read(receiver), Some((list, VarAccess::Move)), "{receiver:?}");
    }
    let [awaited] = shapes.awaited.as_slice() else {
        return Err(format!("expected one await, got {:?}", shapes.awaited));
    };
    assert_eq!(var_read(awaited), Some(("handle", VarAccess::Move)), "{awaited:?}");
    Ok(())
}
