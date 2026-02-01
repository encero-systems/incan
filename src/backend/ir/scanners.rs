//! Feature scanners and collectors for IR codegen
//!
//! This module centralizes feature detection logic. The functions here are pure analyzers over the parsed AST and do
//! not mutate global state.

use std::collections::HashSet;

use crate::frontend::ast::{self, Declaration, Expr, Literal, Program, Spanned, Statement};
use crate::frontend::ast::{CallArg, DecoratorArg, FStringPart, ImportKind};
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::http;
use incan_core::lang::stdlib;

/// Detect whether serde derives are used anywhere in the program
pub fn detect_serde_usage(program: &Program) -> bool {
    for decl in &program.declarations {
        let decorators = match &decl.node {
            Declaration::Model(m) => &m.decorators,
            Declaration::Class(c) => &c.decorators,
            _ => continue,
        };

        for dec in decorators {
            if decorators::from_str(dec.node.name.as_str()) == Some(DecoratorId::Derive) {
                for arg in &dec.node.args {
                    if let DecoratorArg::Positional(expr) = arg {
                        if let Expr::Ident(name) = &expr.node {
                            if matches!(
                                derives::from_str(name.as_str()),
                                Some(DeriveId::Serialize | DeriveId::Deserialize)
                            ) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    // Also enable serde if JSON builtins are used (e.g. `json_stringify(...)`).
    if program_uses_json_stringify(program) {
        return true;
    }
    false
}

/// Detect whether the program uses the `json_stringify` builtin.
fn program_uses_json_stringify(program: &Program) -> bool {
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(func) => {
                if body_uses_json_stringify(&func.body) {
                    return true;
                }
            }
            Declaration::Model(model) => {
                for method in &model.methods {
                    if let Some(body) = &method.node.body {
                        if body_uses_json_stringify(body) {
                            return true;
                        }
                    }
                }
            }
            Declaration::Class(class) => {
                for method in &class.methods {
                    if let Some(body) = &method.node.body {
                        if body_uses_json_stringify(body) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn body_uses_json_stringify(body: &[Spanned<Statement>]) -> bool {
    body.iter().any(|stmt| stmt_uses_json_stringify(&stmt.node))
}

fn stmt_uses_json_stringify(stmt: &Statement) -> bool {
    match stmt {
        Statement::Expr(expr) => expr_uses_json_stringify(&expr.node),
        Statement::Assignment(assign) => expr_uses_json_stringify(&assign.value.node),
        Statement::CompoundAssignment(assign) => expr_uses_json_stringify(&assign.value.node),
        Statement::FieldAssignment(assign) => expr_uses_json_stringify(&assign.value.node),
        Statement::IndexAssignment(assign) => expr_uses_json_stringify(&assign.value.node),
        Statement::TupleUnpack(unpack) => expr_uses_json_stringify(&unpack.value.node),
        Statement::TupleAssign(assign) => expr_uses_json_stringify(&assign.value.node),
        Statement::Return(Some(expr)) => expr_uses_json_stringify(&expr.node),
        Statement::If(if_stmt) => {
            body_uses_json_stringify(&if_stmt.then_body)
                || if_stmt.else_body.as_ref().is_some_and(|b| body_uses_json_stringify(b))
        }
        Statement::While(while_stmt) => body_uses_json_stringify(&while_stmt.body),
        Statement::For(for_stmt) => body_uses_json_stringify(&for_stmt.body),
        _ => false,
    }
}

fn expr_uses_json_stringify(expr: &Expr) -> bool {
    match expr {
        Expr::Call(function, args) => {
            if let Expr::Ident(name) = &function.node {
                if builtins::from_str(name.as_str()) == Some(BuiltinFnId::JsonStringify) {
                    return true;
                }
            }
            expr_uses_json_stringify(&function.node)
                || args.iter().any(|arg| match arg {
                    CallArg::Positional(e) | CallArg::Named(_, e) => expr_uses_json_stringify(&e.node),
                })
        }
        Expr::Binary(left, _, right) => expr_uses_json_stringify(&left.node) || expr_uses_json_stringify(&right.node),
        Expr::Unary(_, expr) => expr_uses_json_stringify(&expr.node),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(|item| expr_uses_json_stringify(&item.node))
        }
        Expr::Dict(pairs) => pairs
            .iter()
            .any(|(k, v)| expr_uses_json_stringify(&k.node) || expr_uses_json_stringify(&v.node)),
        Expr::If(if_expr) => {
            body_uses_json_stringify(&if_expr.then_body)
                || if_expr.else_body.as_ref().is_some_and(|b| body_uses_json_stringify(b))
        }
        Expr::Match(scrutinee, arms) => {
            expr_uses_json_stringify(&scrutinee.node)
                || arms.iter().any(|arm| {
                    let arm = &arm.node;
                    let guard = arm.guard.as_ref().is_some_and(|g| expr_uses_json_stringify(&g.node));
                    let body = match &arm.body {
                        ast::MatchBody::Expr(e) => expr_uses_json_stringify(&e.node),
                        ast::MatchBody::Block(stmts) => body_uses_json_stringify(stmts),
                    };
                    guard || body
                })
        }
        Expr::MethodCall(base, _method, args) => {
            expr_uses_json_stringify(&base.node)
                || args.iter().any(|arg| match arg {
                    CallArg::Positional(e) | CallArg::Named(_, e) => expr_uses_json_stringify(&e.node),
                })
        }
        Expr::Index(base, idx) => expr_uses_json_stringify(&base.node) || expr_uses_json_stringify(&idx.node),
        Expr::Slice(base, slice) => {
            expr_uses_json_stringify(&base.node)
                || slice.start.as_ref().is_some_and(|e| expr_uses_json_stringify(&e.node))
                || slice.end.as_ref().is_some_and(|e| expr_uses_json_stringify(&e.node))
                || slice.step.as_ref().is_some_and(|e| expr_uses_json_stringify(&e.node))
        }
        Expr::Field(base, _field) => expr_uses_json_stringify(&base.node),
        Expr::Range { start, end, .. } => expr_uses_json_stringify(&start.node) || expr_uses_json_stringify(&end.node),
        Expr::Await(inner) => expr_uses_json_stringify(&inner.node),
        Expr::Try(inner) => expr_uses_json_stringify(&inner.node),
        Expr::Paren(inner) => expr_uses_json_stringify(&inner.node),
        Expr::Constructor(_name, args) => args.iter().any(|arg| match arg {
            CallArg::Positional(e) | CallArg::Named(_, e) => expr_uses_json_stringify(&e.node),
        }),
        Expr::ListComp(comp) => {
            expr_uses_json_stringify(&comp.expr.node)
                || expr_uses_json_stringify(&comp.iter.node)
                || comp.filter.as_ref().is_some_and(|f| expr_uses_json_stringify(&f.node))
        }
        Expr::DictComp(comp) => {
            expr_uses_json_stringify(&comp.key.node)
                || expr_uses_json_stringify(&comp.value.node)
                || expr_uses_json_stringify(&comp.iter.node)
                || comp.filter.as_ref().is_some_and(|f| expr_uses_json_stringify(&f.node))
        }
        Expr::Closure(_params, body) => expr_uses_json_stringify(&body.node),
        Expr::FString(parts) => parts.iter().any(|p| match p {
            FStringPart::Literal(_) => false,
            FStringPart::Expr(e) => expr_uses_json_stringify(&e.node),
        }),
        Expr::Yield(Some(e)) => expr_uses_json_stringify(&e.node),
        _ => false,
    }
}

/// Detect whether async runtime is required
pub fn detect_async_usage(program: &Program) -> bool {
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(func) => {
                if func.is_async || body_uses_async(&func.body) {
                    return true;
                }
            }
            Declaration::Model(model) => {
                for method in &model.methods {
                    if method.node.is_async {
                        return true;
                    }
                    if let Some(body) = &method.node.body {
                        if body_uses_async(body) {
                            return true;
                        }
                    }
                }
            }
            Declaration::Class(class) => {
                for method in &class.methods {
                    if method.node.is_async {
                        return true;
                    }
                    if let Some(body) = &method.node.body {
                        if body_uses_async(body) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn body_uses_async(body: &[Spanned<Statement>]) -> bool {
    for stmt in body {
        if stmt_uses_async(&stmt.node) {
            return true;
        }
    }
    false
}

fn stmt_uses_async(stmt: &Statement) -> bool {
    match stmt {
        Statement::Expr(expr) => expr_uses_async(&expr.node),
        Statement::Assignment(assign) => expr_uses_async(&assign.value.node),
        Statement::CompoundAssignment(assign) => expr_uses_async(&assign.value.node),
        Statement::FieldAssignment(assign) => expr_uses_async(&assign.value.node),
        Statement::IndexAssignment(assign) => expr_uses_async(&assign.value.node),
        Statement::TupleUnpack(unpack) => expr_uses_async(&unpack.value.node),
        Statement::TupleAssign(assign) => expr_uses_async(&assign.value.node),
        Statement::Return(Some(expr)) => expr_uses_async(&expr.node),
        Statement::If(if_stmt) => {
            expr_uses_async(&if_stmt.condition.node)
                || body_uses_async(&if_stmt.then_body)
                || if_stmt.else_body.as_ref().is_some_and(|b| body_uses_async(b))
        }
        Statement::While(while_stmt) => {
            expr_uses_async(&while_stmt.condition.node) || body_uses_async(&while_stmt.body)
        }
        Statement::For(for_stmt) => expr_uses_async(&for_stmt.iter.node) || body_uses_async(&for_stmt.body),
        _ => false,
    }
}

fn expr_uses_async(expr: &Expr) -> bool {
    match expr {
        Expr::Await(_) => true,
        Expr::Call(function, args) => {
            if let Expr::Ident(name) = &function.node {
                if let Some(id) = incan_core::lang::surface::functions::from_str(name.as_str()) {
                    use incan_core::lang::surface::functions::SurfaceFnId as F;
                    return matches!(
                        id,
                        F::SleepMs
                            | F::Timeout
                            | F::TimeoutMs
                            | F::SelectTimeout
                            | F::YieldNow
                            | F::Spawn
                            | F::SpawnBlocking
                            | F::Channel
                            | F::UnboundedChannel
                            | F::Oneshot
                    );
                }
                if incan_core::lang::builtins::from_str(name.as_str())
                    == Some(incan_core::lang::builtins::BuiltinFnId::Sleep)
                {
                    return true;
                }
            }
            expr_uses_async(&function.node) || args.iter().any(call_arg_uses_async)
        }
        Expr::Binary(left, _, right) => expr_uses_async(&left.node) || expr_uses_async(&right.node),
        Expr::Unary(_, expr) => expr_uses_async(&expr.node),
        Expr::MethodCall(receiver, _, args) => expr_uses_async(&receiver.node) || args.iter().any(call_arg_uses_async),
        Expr::Field(base, _) => expr_uses_async(&base.node),
        Expr::Index(base, index) => expr_uses_async(&base.node) || expr_uses_async(&index.node),
        Expr::Slice(base, _) => expr_uses_async(&base.node),
        Expr::If(if_expr) => {
            expr_uses_async(&if_expr.condition.node)
                || body_uses_async(&if_expr.then_body)
                || if_expr.else_body.as_ref().is_some_and(|b| body_uses_async(b))
        }
        Expr::Match(expr, arms) => {
            expr_uses_async(&expr.node) || arms.iter().any(|arm| match_body_uses_async(&arm.node.body))
        }
        Expr::Closure(_, body) => expr_uses_async(&body.node),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(|item| expr_uses_async(&item.node))
        }
        Expr::Dict(pairs) => pairs
            .iter()
            .any(|(k, v)| expr_uses_async(&k.node) || expr_uses_async(&v.node)),
        Expr::FString(parts) => parts
            .iter()
            .any(|part| matches!(part, FStringPart::Expr(e) if expr_uses_async(&e.node))),
        Expr::ListComp(comp) => {
            expr_uses_async(&comp.expr.node)
                || expr_uses_async(&comp.iter.node)
                || comp.filter.as_ref().is_some_and(|c| expr_uses_async(&c.node))
        }
        Expr::DictComp(comp) => {
            expr_uses_async(&comp.key.node)
                || expr_uses_async(&comp.value.node)
                || expr_uses_async(&comp.iter.node)
                || comp.filter.as_ref().is_some_and(|c| expr_uses_async(&c.node))
        }
        Expr::Constructor(_, args) => args.iter().any(call_arg_uses_async),
        Expr::Try(inner) => expr_uses_async(&inner.node),
        Expr::Paren(inner) => expr_uses_async(&inner.node),
        _ => false,
    }
}

fn call_arg_uses_async(arg: &CallArg) -> bool {
    match arg {
        CallArg::Positional(expr) => expr_uses_async(&expr.node),
        CallArg::Named(_, expr) => expr_uses_async(&expr.node),
    }
}

fn match_body_uses_async(body: &ast::MatchBody) -> bool {
    match body {
        ast::MatchBody::Expr(expr) => expr_uses_async(&expr.node),
        ast::MatchBody::Block(stmts) => body_uses_async(stmts),
    }
}

/// Detect web framework usage (axum/tokio/serde implied)
pub fn detect_web_usage(program: &Program) -> bool {
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Import(import) => match &import.kind {
                ImportKind::Module(path) if !path.segments.is_empty() => {
                    if path.segments[0] == stdlib::STDLIB_WEB {
                        return true;
                    }
                }
                ImportKind::From { module, .. } if !module.segments.is_empty() => {
                    if module.segments[0] == stdlib::STDLIB_WEB {
                        return true;
                    }
                }
                _ => {}
            },
            Declaration::Function(func) => {
                if func
                    .decorators
                    .iter()
                    .any(|d| decorators::from_str(d.node.name.as_str()) == Some(DecoratorId::Route))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Detect list helper usage (remove, count, index)
pub fn detect_list_helpers_usage(program: &Program) -> bool {
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(func) => {
                if body_uses_list_helpers(&func.body) {
                    return true;
                }
            }
            Declaration::Model(model) => {
                for method in &model.methods {
                    if let Some(body) = &method.node.body {
                        if body_uses_list_helpers(body) {
                            return true;
                        }
                    }
                }
            }
            Declaration::Class(class) => {
                for method in &class.methods {
                    if let Some(body) = &method.node.body {
                        if body_uses_list_helpers(body) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn body_uses_list_helpers(body: &[Spanned<Statement>]) -> bool {
    body.iter().any(|stmt| stmt_uses_list_helpers(&stmt.node))
}

fn stmt_uses_list_helpers(stmt: &Statement) -> bool {
    match stmt {
        Statement::Expr(expr) => expr_uses_list_helpers(&expr.node),
        Statement::Assignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::CompoundAssignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::FieldAssignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::IndexAssignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::TupleUnpack(unpack) => expr_uses_list_helpers(&unpack.value.node),
        Statement::TupleAssign(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::Return(Some(expr)) => expr_uses_list_helpers(&expr.node),
        Statement::If(if_stmt) => {
            body_uses_list_helpers(&if_stmt.then_body)
                || if_stmt.else_body.as_ref().is_some_and(|b| body_uses_list_helpers(b))
        }
        Statement::While(while_stmt) => body_uses_list_helpers(&while_stmt.body),
        Statement::For(for_stmt) => body_uses_list_helpers(&for_stmt.body),
        _ => false,
    }
}

fn expr_uses_list_helpers(expr: &Expr) -> bool {
    match expr {
        Expr::MethodCall(_base, method, _args) => {
            matches!(method.as_str(), "remove" | "count" | "index")
        }
        Expr::Call(function, args) => {
            expr_uses_list_helpers(&function.node)
                || args.iter().any(|arg| match arg {
                    CallArg::Positional(e) | CallArg::Named(_, e) => expr_uses_list_helpers(&e.node),
                })
        }
        Expr::Binary(left, _, right) => expr_uses_list_helpers(&left.node) || expr_uses_list_helpers(&right.node),
        Expr::Unary(_, expr) => expr_uses_list_helpers(&expr.node),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(|item| expr_uses_list_helpers(&item.node))
        }
        Expr::If(if_expr) => {
            body_uses_list_helpers(&if_expr.then_body)
                || if_expr.else_body.as_ref().is_some_and(|b| body_uses_list_helpers(b))
        }
        _ => false,
    }
}

/// Collected route metadata: (handler, path, methods, unknown_methods, is_async, module_path_segments).
pub type RouteScan = (
    String,
    String,
    Vec<http::HttpMethodId>,
    Vec<String>,
    bool,
    Option<Vec<String>>,
);

/// Collect routes from `@route` decorators.
///
/// The `module_path_segments` parameter should be `None` for the main module, or `Some(&["api", "routes"])`
/// for nested submodules.
pub fn collect_routes(program: &Program, module_path_segments: Option<&[String]>) -> Vec<RouteScan> {
    let mut routes = Vec::new();
    for decl in &program.declarations {
        if let Declaration::Function(func) = &decl.node {
            for dec in &func.decorators {
                if decorators::from_str(dec.node.name.as_str()) == Some(DecoratorId::Route) {
                    let mut path = String::new();
                    let mut methods = vec![http::HttpMethodId::Get];
                    let mut unknown_methods: Vec<String> = Vec::new();
                    for arg in &dec.node.args {
                        match arg {
                            DecoratorArg::Positional(expr) => {
                                if path.is_empty() {
                                    if let Expr::Literal(Literal::String(s)) = &expr.node {
                                        path = s.clone();
                                    }
                                }
                            }
                            DecoratorArg::Named(name, value) => {
                                if name == decorators::ROUTE_METHODS_ARG {
                                    if let ast::DecoratorArgValue::Expr(expr) = value {
                                        if let Expr::List(items) = &expr.node {
                                            let mut method_strings = Vec::new();
                                            for item in items {
                                                match &item.node {
                                                    Expr::Literal(Literal::String(s)) => {
                                                        method_strings.push(s.clone());
                                                    }
                                                    Expr::Ident(name) => {
                                                        method_strings.push(name.clone());
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            if !method_strings.is_empty() {
                                                // Note: only the first recognized method is used today.
                                                let mut selected: Option<http::HttpMethodId> = None;
                                                for method in method_strings {
                                                    if let Some(id) = http::from_str(method.as_str()) {
                                                        if selected.is_none() {
                                                            selected = Some(id);
                                                        }
                                                    } else {
                                                        unknown_methods.push(method);
                                                    }
                                                }
                                                methods = selected.into_iter().collect();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !path.is_empty() {
                        routes.push((
                            func.name.clone(),
                            path,
                            methods,
                            unknown_methods,
                            func.is_async,
                            module_path_segments.map(|segs| segs.to_vec()),
                        ));
                    }
                }
            }
        }
    }
    routes
}

/// Collect Rust crates imported via `import rust::` or `from rust::`
pub fn collect_rust_crates(program: &Program) -> HashSet<String> {
    let mut crates = HashSet::new();
    for decl in &program.declarations {
        if let Declaration::Import(import) = &decl.node {
            match &import.kind {
                ImportKind::RustCrate { crate_name, .. } => {
                    if crate_name != stdlib::STDLIB_ROOT {
                        crates.insert(crate_name.clone());
                    }
                }
                ImportKind::RustFrom { crate_name, .. } => {
                    if crate_name != stdlib::STDLIB_ROOT {
                        crates.insert(crate_name.clone());
                    }
                }
                _ => {}
            }
        }
    }
    crates
}

/// Check for `import this` usage
pub fn check_for_this_import(program: &Program) -> bool {
    for decl in &program.declarations {
        if let Declaration::Import(import) = &decl.node {
            if let ImportKind::Module(path) = &import.kind {
                if path.segments.len() == 1 && path.segments[0] == stdlib::STDLIB_THIS {
                    return true;
                }
            }
        }
    }
    false
}
