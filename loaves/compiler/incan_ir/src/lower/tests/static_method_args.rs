//! Builtin-family method calls as lowering hands them to the emitter: an argument of a method on module static storage
//! that the program reads again reaches the call as a view or a copy of itself, a value at its last read as it is
//! (#1793); and a dict's `get` answers with the stored value on every dict.

use super::*;
use crate::expr::{IrInteropCoercionKind, VarAccess};
use crate::stmt::AssignTarget;

/// Return the body of the named function.
fn function_body<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [IrStmt], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function.body.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Collect every builtin-family method call in a function body, in evaluation order of the IR, with its arguments.
fn known_method_calls<'a>(ir: &'a IrProgram, name: &str) -> Result<Vec<(&'a MethodKind, &'a [IrCallArg])>, String> {
    let mut calls = Vec::new();
    for stmt in function_body(ir, name)? {
        collect_known_calls_in_stmt(stmt, &mut calls);
    }
    Ok(calls)
}

/// Walk one statement for builtin-family method calls.
fn collect_known_calls_in_stmt<'a>(stmt: &'a IrStmt, calls: &mut Vec<(&'a MethodKind, &'a [IrCallArg])>) {
    match &stmt.kind {
        IrStmtKind::Expr(expr)
        | IrStmtKind::Return(Some(expr))
        | IrStmtKind::Assign { value: expr, .. }
        | IrStmtKind::Let { value: expr, .. } => collect_known_calls_in_expr(expr, calls),
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_known_calls_in_expr(condition, calls);
            for stmt in then_branch.iter().chain(else_branch.iter().flatten()) {
                collect_known_calls_in_stmt(stmt, calls);
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            collect_known_calls_in_expr(scrutinee, calls);
            for arm in arms {
                collect_known_calls_in_expr(&arm.body, calls);
            }
        }
        IrStmtKind::For { body, .. } | IrStmtKind::Block(body) => {
            for stmt in body {
                collect_known_calls_in_stmt(stmt, calls);
            }
        }
        _ => {}
    }
}

/// Walk one expression for builtin-family method calls.
fn collect_known_calls_in_expr<'a>(expr: &'a TypedExpr, calls: &mut Vec<(&'a MethodKind, &'a [IrCallArg])>) {
    match &expr.kind {
        IrExprKind::KnownMethodCall { kind, args, .. } => calls.push((kind, args.as_slice())),
        IrExprKind::MethodCall { receiver, .. } => collect_known_calls_in_expr(receiver, calls),
        IrExprKind::UnaryOp { operand, .. } => collect_known_calls_in_expr(operand, calls),
        IrExprKind::BinOp { left, right, .. } => {
            collect_known_calls_in_expr(left, calls);
            collect_known_calls_in_expr(right, calls);
        }
        IrExprKind::Match { scrutinee, arms } => {
            collect_known_calls_in_expr(scrutinee, calls);
            for arm in arms {
                collect_known_calls_in_expr(&arm.body, calls);
            }
        }
        IrExprKind::Block { stmts, value } => {
            for stmt in stmts {
                collect_known_calls_in_stmt(stmt, calls);
            }
            if let Some(value) = value {
                collect_known_calls_in_expr(value, calls);
            }
        }
        _ => {}
    }
}

/// Name what one argument expression reads: a variable by its name, a field as `.field`.
fn place_name(expr: &TypedExpr) -> String {
    match &expr.kind {
        IrExprKind::Var { name, .. } => name.clone(),
        IrExprKind::Field { field, .. } => format!(".{field}"),
        other => format!("{other:?}"),
    }
}

/// Describe how one argument reaches the call: `copy of <place>`, `view of <place>`, `<place>` as it is, a string
/// literal with its type, or another expression kind.
fn argument_shape(arg: &IrCallArg) -> String {
    match &arg.expr.kind {
        IrExprKind::MethodCall {
            receiver, method, args, ..
        } if method == "clone" && args.is_empty() => format!("copy of {}", place_name(receiver)),
        IrExprKind::InteropCoerce {
            expr,
            kind: IrInteropCoercionKind::RustBorrow { mutable: false },
            ..
        } => format!("view of {}", place_name(expr)),
        IrExprKind::Var { .. } | IrExprKind::Field { .. } => place_name(&arg.expr),
        IrExprKind::Int(value) => value.to_string(),
        IrExprKind::String(value) => format!("{value:?}: {:?}", arg.expr.ty),
        other => format!("{other:?}"),
    }
}

/// The argument shapes of every builtin-family call in the named function, call by call.
fn call_argument_shapes(ir: &IrProgram, function: &str) -> Result<Vec<Vec<String>>, String> {
    Ok(known_method_calls(ir, function)?
        .into_iter()
        .map(|(_, args)| args.iter().map(argument_shape).collect())
        .collect())
}

/// #1793: an argument of a builtin method on static storage that the program reads again is handed over as a view of
/// itself when the method reads it in place (a `str` key or element for `get`, `in` and `contains_key`, a prefix for
/// `startswith`, a list for `extend`), and as a copy when the method stores it (`insert`, `append`, `add`). A value
/// at its last read goes in as it is, and a string literal goes in typed as the `'static` string it is, so a method
/// that stores it makes the static's owned `str` from it.
#[test]
fn arguments_read_after_a_static_method_call_are_kept_readable_issue1793() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
model User:
    name: str


static counts: dict[str, int] = {}
static names: list[str] = []
static tags: set[str] = Set()
static greeting: str = "hello"
static items: list[int] = []


def lookup(name: str) -> str:
    match counts.get(name):
        Some(value) => return f"{name}={value}"
        None => return f"{name} missing"


def record(name: str) -> str:
    counts.insert(name, 2)
    if name in counts:
        return name
    if counts.contains_key(name):
        return name
    return name


def listed(name: str) -> str:
    names.append(name)
    tags.add(name)
    if greeting.startswith(name):
        return name
    return name


def by_user(user: User) -> str:
    match counts.get(user.name):
        Some(value) => return f"{value}"
        None => return user.name


def grow(extra: list[int]) -> int:
    items.extend(extra)
    return len(extra)


def last_read(name: str) -> Option[int]:
    return counts.get(name)


def seed() -> None:
    counts.insert("a", 1)
    names.append("b")
"#,
    )?;

    for (function, expected) in [
        ("lookup", vec![vec!["view of name"]]),
        (
            "record",
            vec![vec!["copy of name", "2"], vec!["view of name"], vec!["view of name"]],
        ),
        (
            "listed",
            vec![vec!["copy of name"], vec!["copy of name"], vec!["view of name"]],
        ),
        ("by_user", vec![vec!["view of .name"]]),
        ("grow", vec![vec!["view of extra"]]),
        ("last_read", vec![vec!["name"]]),
        ("seed", vec![vec!["\"a\": StaticStr", "1"], vec!["\"b\": StaticStr"]]),
    ] {
        assert_eq!(
            call_argument_shapes(&ir, function)?,
            expected,
            "`{function}` hands its static method calls these arguments"
        );
    }
    Ok(())
}

/// #1793, read order: the generated code evaluates a static index assignment's value before its key, and binds a
/// method's argument before it reads the receiver's own path. So `counts[name] = counts.get(name)...` reads `name` in
/// `get` before the key read, and `graph[node].contains(node)` reads `node` in the path after the argument: in both,
/// the argument is kept as a view although it is the variable's last read in source order. Inside a loop a local key
/// is viewed rather than copied (the loop's first call is `strip`, with no argument).
#[test]
fn arguments_read_again_by_the_generated_code_are_kept_readable_issue1793() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
static counts: dict[str, int] = {}
static graph: dict[str, list[str]] = {}
static seen: set[str] = Set()


def record(name: str) -> None:
    counts[name] = counts.get(name).unwrap_or(0) + 1


def linked(node: str) -> bool:
    return graph[node].contains(node)


def looped(lines: list[str]) -> int:
    mut total = 0
    for line in lines:
        key = line.strip()
        if key in seen:
            total += counts.get(key).unwrap_or(0)
    return total
"#,
    )?;

    for (function, expected) in [
        ("record", vec![vec!["view of name"]]),
        ("linked", vec![vec!["view of node"]]),
        ("looped", vec![vec![], vec!["view of key"], vec!["view of key"]]),
    ] {
        assert_eq!(
            call_argument_shapes(&ir, function)?,
            expected,
            "`{function}` hands its static method calls these arguments"
        );
    }

    // The key of the assignment is the name's last read, and it is read after the value.
    let [.., last] = function_body(&ir, "record")? else {
        return Err("`record` has no statements".to_string());
    };
    let IrStmtKind::Assign {
        target: AssignTarget::Index { index, .. },
        ..
    } = &last.kind
    else {
        return Err(format!("`record` must end in an index assignment, got {last:?}"));
    };
    assert!(
        matches!(
            &index.kind,
            IrExprKind::Var {
                access: VarAccess::Move,
                ..
            }
        ),
        "the key is the last read of `name`, got {index:?}"
    );
    Ok(())
}

/// The preparation is specific to static storage: the same call on a local dict keeps its argument as written, and a
/// view keeps the argument's own type so the storage access reads it as it reads any other argument.
#[test]
fn arguments_of_a_local_collection_method_are_left_as_written_issue1793() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
static counts: dict[str, int] = {}


def local(table: dict[str, int], name: str) -> str:
    match table.get(name):
        Some(value) => return f"{name}={value}"
        None => return name


def viewed(name: str) -> str:
    match counts.get(name):
        Some(value) => return f"{name}={value}"
        None => return name
"#,
    )?;

    assert_eq!(call_argument_shapes(&ir, "local")?, vec![vec!["name".to_string()]]);

    let viewed = known_method_calls(&ir, "viewed")?;
    let [(MethodKind::Collection(CollectionMethodKind::Get), [arg])] = viewed.as_slice() else {
        return Err(format!(
            "`viewed` must make one `get` call with one argument, got {viewed:?}"
        ));
    };
    assert_eq!(argument_shape(arg), "view of name");
    assert_eq!(arg.expr.ty, IrType::String, "the view keeps the argument's type");
    Ok(())
}

/// `get` answers with the stored value on every dict. A dict that is not static storage is read in place, so lowering
/// completes the lookup with `copied` for a `Copy` value and `cloned` otherwise; a static dict, or a local bound to
/// one, is read through the storage access, which already copies the entry out.
#[test]
fn dict_get_answers_with_the_stored_value() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
model Point:
    x: int


static counts: dict[str, int] = {}


def scalar(table: dict[str, int], name: str) -> Option[int]:
    return table.get(name)


def model_value(table: dict[str, Point], name: str) -> Option[Point]:
    return table.get(name)


def static_value(name: str) -> Option[int]:
    return counts.get(name)


def aliased(name: str) -> Option[int]:
    live = counts
    return live.get(name)
"#,
    )?;

    for (function, completion) in [
        ("scalar", Some("copied")),
        ("model_value", Some("cloned")),
        ("static_value", None),
        ("aliased", None),
    ] {
        let Some(IrStmt {
            kind: IrStmtKind::Return(Some(returned)),
            ..
        }) = function_body(&ir, function)?.last()
        else {
            return Err(format!("`{function}` must end in a return"));
        };
        let lookup = match (&returned.kind, completion) {
            (IrExprKind::MethodCall { receiver, method, .. }, Some(expected)) => {
                assert_eq!(method, expected, "`{function}` completes its lookup with `{expected}`");
                receiver.as_ref()
            }
            (_, None) => returned,
            (other, Some(expected)) => {
                return Err(format!(
                    "`{function}` must complete its lookup with `{expected}`, got {other:?}"
                ));
            }
        };
        assert!(
            matches!(
                lookup.kind,
                IrExprKind::KnownMethodCall {
                    kind: MethodKind::Collection(CollectionMethodKind::Get),
                    ..
                }
            ),
            "`{function}` looks the key up with `get`, got {lookup:?}"
        );
        assert!(
            matches!(&returned.ty, IrType::Option(inner) if !matches!(inner.as_ref(), IrType::Ref(_))),
            "`{function}` answers with the stored value, got {:?}",
            returned.ty
        );
    }
    Ok(())
}

/// Return the arguments of the first ordinary method call named `method` in a function body's statements.
fn method_call_args<'a>(ir: &'a IrProgram, function: &str, method: &str) -> Result<&'a [IrCallArg], String> {
    fn find<'a>(expr: &'a TypedExpr, method: &str) -> Option<&'a [IrCallArg]> {
        match &expr.kind {
            IrExprKind::MethodCall { method: name, args, .. } if name == method => Some(args.as_slice()),
            _ => None,
        }
    }
    function_body(ir, function)?
        .iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Expr(expr) => find(expr, method),
            IrStmtKind::If { condition, .. } => find(condition, method),
            _ => None,
        })
        .ok_or_else(|| format!("`{function}` makes no `{method}` call"))
}

/// #1793 on a `rust::` collection: a static `BTreeMap` is called through ordinary method calls, and a key the program
/// reads again is handed over as a copy of itself, as it is for a builtin method that stores its argument.
#[test]
fn arguments_of_a_rust_collection_method_on_a_static_are_copied_issue1793() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from rust::std::collections import BTreeMap

static index: BTreeMap[str, int] = BTreeMap.new()


def seen(key: str) -> str:
    if index.contains_key(key):
        return key
    index.insert(key, 1)
    return key
"#,
    )?;
    for method in ["contains_key", "insert"] {
        let args = method_call_args(&ir, "seen", method)?;
        assert_eq!(
            args.first().map(argument_shape).as_deref(),
            Some("copy of key"),
            "`{method}` on the static `BTreeMap` copies the reused key"
        );
    }
    Ok(())
}
