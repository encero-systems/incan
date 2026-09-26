//! Arguments of a builtin method on module static storage are handed over readable: a value the program reads again
//! after the call reaches the call as a copy of itself, a value at its last read as it is (#1793).

use super::*;

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

/// Collect every builtin-family method call in a function body, in source order, with its arguments.
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
        IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) => collect_known_calls_in_expr(expr, calls),
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
        IrStmtKind::Block(stmts) => {
            for stmt in stmts {
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

/// Describe how one argument reaches the call: `copy of <name>`, `<name>` as it is, a string literal with its type, or
/// another expression kind.
fn argument_shape(arg: &IrCallArg) -> String {
    match &arg.expr.kind {
        IrExprKind::MethodCall {
            receiver, method, args, ..
        } if method == "clone" && args.is_empty() => match &receiver.kind {
            IrExprKind::Var { name, .. } => format!("copy of {name}"),
            IrExprKind::Field { field, .. } => format!("copy of .{field}"),
            other => format!("copy of {other:?}"),
        },
        IrExprKind::Var { name, .. } => name.clone(),
        IrExprKind::Field { field, .. } => format!(".{field}"),
        IrExprKind::Int(value) => value.to_string(),
        IrExprKind::String(value) => format!("{value:?}: {:?}", arg.expr.ty),
        other => format!("{other:?}"),
    }
}

/// #1793: `counts.get(name)` on a static dict, with `name` read again in the match arms, hands the lookup a copy of
/// `name`; the emitter binds that copy to its temporary before the static's storage access, so `name` stays readable.
/// Every builtin method on static storage follows the same rule: `insert`, `in`, `contains_key`, `append`, `add`, a
/// `str` method on a static string, and a field of another value. A value at its last read goes in as it is, and a
/// string literal goes in typed as the `'static` string it is, so a method that stores it makes the static's owned
/// `str` from it.
#[test]
fn arguments_read_after_a_static_method_call_are_copied_in_issue1793() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
model User:
    name: str


static counts: dict[str, int] = {}
static names: list[str] = []
static tags: set[str] = Set()
static greeting: str = "hello"


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


def last_read(name: str) -> Option[int]:
    return counts.get(name)


def seed() -> None:
    counts.insert("a", 1)
    names.append("b")
"#,
    )?;

    for (function, expected) in [
        ("lookup", vec![vec!["copy of name"]]),
        (
            "record",
            vec![vec!["copy of name", "2"], vec!["copy of name"], vec!["copy of name"]],
        ),
        (
            "listed",
            vec![vec!["copy of name"], vec!["copy of name"], vec!["copy of name"]],
        ),
        ("by_user", vec![vec!["copy of .name"]]),
        ("last_read", vec![vec!["name"]]),
        ("seed", vec![vec!["\"a\": StaticStr", "1"], vec!["\"b\": StaticStr"]]),
    ] {
        let shapes: Vec<Vec<String>> = known_method_calls(&ir, function)?
            .into_iter()
            .map(|(_, args)| args.iter().map(argument_shape).collect())
            .collect();
        assert_eq!(
            shapes, expected,
            "`{function}` hands its static method calls these arguments"
        );
    }
    Ok(())
}

/// The copy is specific to static storage: the same calls on a local collection keep their arguments as written, and
/// the copy keeps the argument's own type so the storage access reads it as it reads any other argument.
#[test]
fn arguments_of_a_local_collection_method_are_left_as_written_issue1793() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
static counts: dict[str, int] = {}


def local(table: dict[str, int], name: str) -> str:
    match table.get(name):
        Some(value) => return f"{name}={value}"
        None => return name


def copied(name: str) -> str:
    match counts.get(name):
        Some(value) => return f"{name}={value}"
        None => return name
"#,
    )?;

    let local = known_method_calls(&ir, "local")?;
    let local_shapes: Vec<String> = local
        .iter()
        .flat_map(|(_, args)| args.iter().map(argument_shape))
        .collect();
    assert_eq!(local_shapes, vec!["name".to_string()]);

    let copied = known_method_calls(&ir, "copied")?;
    let [(MethodKind::Collection(CollectionMethodKind::Get), [arg])] = copied.as_slice() else {
        return Err(format!(
            "`copied` must make one `get` call with one argument, got {copied:?}"
        ));
    };
    assert_eq!(argument_shape(arg), "copy of name");
    assert_eq!(arg.expr.ty, IrType::String, "the copy keeps the argument's type");
    Ok(())
}
