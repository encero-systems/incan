//! Serde/JSON feature detection.
//!
//! Activation is primarily import-driven (RFC 022): importing from `std.serde` signals
//! that serde is required.
//!
//! We also check for `@derive(Serialize/Deserialize)` decorators (explicit opt-in) and
//! for bare `json_stringify()` calls (legacy builtin that doesn't yet require an import).
//! Once `json_stringify` is behind `from std.serde.json import json_stringify`, the
//! builtin fallback here can be removed entirely.

use crate::frontend::ast::{CallArg, Declaration, DecoratorArg, Expr, Program, Spanned, Statement};
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::decorators::DecoratorId;
use incan_core::lang::derives::{self, DeriveId};

use super::decorators::{collect_import_aliases, has_stdlib_import, resolve_decorator_id};

/// Detect whether serde derives are used anywhere in the program.
pub fn detect_serde_usage(program: &Program) -> bool {
    // Fast path: explicit `import std.serde.json` or `from std.serde import ...`
    if has_stdlib_import(program, "serde") {
        return true;
    }

    // Check for `@derive(Serialize)` / `@derive(Deserialize)` on models/classes.
    if has_serde_derive(program) {
        return true;
    }

    // TODO: Remove this legacy fallback once `json_stringify` requires an explicit import
    // (e.g. `from std.serde.json import json_stringify`). Until then, bare calls activate serde
    // without an import, which breaks the "imported module → activate its features" invariant
    // that the rest of the stdlib follows.
    program_has_json_stringify(program)
}

/// Check for `@derive(Serialize/Deserialize)` on any model or class.
fn has_serde_derive(program: &Program) -> bool {
    let aliases = collect_import_aliases(program);
    for decl in &program.declarations {
        let decorators = match &decl.node {
            Declaration::Model(m) => &m.decorators,
            Declaration::Class(c) => &c.decorators,
            _ => continue,
        };

        for dec in decorators {
            if resolve_decorator_id(&dec.node, &aliases) != Some(DecoratorId::Derive) {
                continue;
            }
            for arg in &dec.node.args {
                let DecoratorArg::Positional(expr) = arg else { continue };
                let Expr::Ident(name) = &expr.node else { continue };
                if matches!(
                    derives::from_str(name.as_str()),
                    Some(DeriveId::Serialize | DeriveId::Deserialize)
                ) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Legacy `json_stringify()` builtin detection
//
// This will be removable once json_stringify requires `from std.serde.json import ...`.
// ---------------------------------------------------------------------------

fn program_has_json_stringify(program: &Program) -> bool {
    for decl in &program.declarations {
        let bodies: Vec<&[Spanned<Statement>]> = match &decl.node {
            Declaration::Function(f) => vec![&f.body],
            Declaration::Model(m) => m.methods.iter().filter_map(|m| m.node.body.as_deref()).collect(),
            Declaration::Class(c) => c.methods.iter().filter_map(|m| m.node.body.as_deref()).collect(),
            _ => continue,
        };
        for body in bodies {
            if body_has_call_named(body, BuiltinFnId::JsonStringify) {
                return true;
            }
        }
    }
    false
}

fn body_has_call_named(body: &[Spanned<Statement>], target: BuiltinFnId) -> bool {
    body.iter().any(|s| stmt_has_call(s, target))
}

fn stmt_has_call(stmt: &Spanned<Statement>, target: BuiltinFnId) -> bool {
    match &stmt.node {
        Statement::Expr(e) | Statement::Return(Some(e)) => expr_has_call(&e.node, target),
        Statement::Assignment(a) => expr_has_call(&a.value.node, target),
        Statement::CompoundAssignment(a) => expr_has_call(&a.value.node, target),
        Statement::FieldAssignment(a) => expr_has_call(&a.value.node, target),
        Statement::IndexAssignment(a) => expr_has_call(&a.value.node, target),
        Statement::TupleUnpack(u) => expr_has_call(&u.value.node, target),
        Statement::TupleAssign(a) => expr_has_call(&a.value.node, target),
        Statement::Surface(surface_stmt) => match &surface_stmt.payload {
            crate::frontend::ast::SurfaceStmtPayload::KeywordArgs(args) => {
                args.iter().any(|arg| expr_has_call(&arg.node, target))
            }
        },
        Statement::VocabBlock(block) => {
            block.header_args.iter().any(|arg| expr_has_call(&arg.node, target))
                || body_has_call_named(&block.body, target)
        }
        Statement::If(s) => {
            body_has_call_named(&s.then_body, target)
                || s.else_body.as_ref().is_some_and(|b| body_has_call_named(b, target))
        }
        Statement::While(s) => body_has_call_named(&s.body, target),
        Statement::For(s) => body_has_call_named(&s.body, target),
        _ => false,
    }
}

/// Compact recursive expression check — does any sub-expression call the target builtin?
fn expr_has_call(expr: &Expr, target: BuiltinFnId) -> bool {
    match expr {
        Expr::Call(f, args) => {
            if let Expr::Ident(name) = &f.node
                && builtins::from_str(name.as_str()) == Some(target)
            {
                return true;
            }
            expr_has_call(&f.node, target) || args.iter().any(|a| call_arg_has(a, target))
        }
        Expr::MethodCall(base, _, args) => {
            expr_has_call(&base.node, target) || args.iter().any(|a| call_arg_has(a, target))
        }
        Expr::Constructor(_, args) => args.iter().any(|a| call_arg_has(a, target)),
        Expr::Binary(l, _, r) | Expr::Index(l, r) => expr_has_call(&l.node, target) || expr_has_call(&r.node, target),
        Expr::Range { start, end, .. } => expr_has_call(&start.node, target) || expr_has_call(&end.node, target),
        Expr::Unary(_, e) | Expr::Try(e) | Expr::Paren(e) | Expr::Field(e, _) => expr_has_call(&e.node, target),
        Expr::Surface(surface_expr) => match &surface_expr.payload {
            crate::frontend::ast::SurfaceExprPayload::PrefixUnary(inner) => expr_has_call(&inner.node, target),
        },
        Expr::Closure(_, body) => expr_has_call(&body.node, target),
        Expr::Yield(Some(e)) => expr_has_call(&e.node, target),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(|i| expr_has_call(&i.node, target))
        }
        Expr::Dict(pairs) => pairs
            .iter()
            .any(|(k, v)| expr_has_call(&k.node, target) || expr_has_call(&v.node, target)),
        Expr::If(if_expr) => {
            body_has_call_named(&if_expr.then_body, target)
                || if_expr
                    .else_body
                    .as_ref()
                    .is_some_and(|b| body_has_call_named(b, target))
        }
        Expr::Match(scrutinee, arms) => {
            expr_has_call(&scrutinee.node, target)
                || arms.iter().any(|arm| match &arm.node.body {
                    crate::frontend::ast::MatchBody::Expr(e) => expr_has_call(&e.node, target),
                    crate::frontend::ast::MatchBody::Block(stmts) => body_has_call_named(stmts, target),
                })
        }
        Expr::Slice(base, slice) => {
            expr_has_call(&base.node, target)
                || slice.start.as_ref().is_some_and(|e| expr_has_call(&e.node, target))
                || slice.end.as_ref().is_some_and(|e| expr_has_call(&e.node, target))
                || slice.step.as_ref().is_some_and(|e| expr_has_call(&e.node, target))
        }
        Expr::ListComp(c) => {
            expr_has_call(&c.expr.node, target)
                || expr_has_call(&c.iter.node, target)
                || c.filter.as_ref().is_some_and(|f| expr_has_call(&f.node, target))
        }
        Expr::DictComp(c) => {
            expr_has_call(&c.key.node, target)
                || expr_has_call(&c.value.node, target)
                || expr_has_call(&c.iter.node, target)
                || c.filter.as_ref().is_some_and(|f| expr_has_call(&f.node, target))
        }
        Expr::FString(parts) => parts.iter().any(|p| match p {
            crate::frontend::ast::FStringPart::Literal(_) => false,
            crate::frontend::ast::FStringPart::Expr(e) => expr_has_call(&e.node, target),
        }),
        _ => false,
    }
}

fn call_arg_has(arg: &CallArg, target: BuiltinFnId) -> bool {
    match arg {
        CallArg::Positional(e) | CallArg::Named(_, e) => expr_has_call(&e.node, target),
    }
}
