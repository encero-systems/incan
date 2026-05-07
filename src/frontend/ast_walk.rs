//! Shared expression traversal over the frontend AST.
//!
//! This module is the canonical place to recurse through expression-bearing AST slots.
//! Callers provide a predicate and receive `true` on the first match.
//!
//! ## Traversal semantics
//! - Source-order traversal.
//! - Deterministic behavior (same AST + predicate => same visit order/result).
//! - Short-circuit evaluation: traversal stops at the first `true`.
//!
//! ## Coverage contract
//! The traversal visits expression slots in declarations, statements, and expressions, including:
//! - declaration values/defaults (`const`/`static`, field defaults, parameter defaults)
//! - decorator args (positional and expression-valued named args)
//! - control-flow conditions and bodies (`if`/`elif`/`else`, `while`, `for`)
//! - assignment RHS and lvalue expressions where applicable (for example tuple/index/field forms)
//! - call/constructor args, match guards and bodies, comprehensions, and f-string interpolations
//! - newtype rebinding/interop adapter expressions
//!
//! ## Non-goals
//! - type traversal (for example type annotations and type arguments)
//! - pattern traversal
//! - import-path traversal

use crate::frontend::ast::{
    AssertKind, CallArg, ComprehensionClause, Condition, Declaration, DecoratorArg, DecoratorArgValue, DictEntry, Expr,
    ListEntry, MatchBody, Program, Spanned, Statement,
};

/// Returns `true` if any expression in `program` satisfies `pred`.
///
/// This entry point walks expression-bearing declaration content and then descends into contained statement/expression
/// trees using the same ordering and early-exit semantics documented at the module level.
pub(crate) fn any_expr_in_program<F>(program: &Program, mut pred: F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    program
        .declarations
        .iter()
        .any(|decl| declaration_has_expr(decl, &mut pred))
}

/// Returns `true` if any expression reachable from `body` satisfies `pred`.
///
/// Use this when a caller already has a statement slice (for example function/method bodies) and does not need to walk
/// top-level declarations.
pub(crate) fn any_expr_in_body<F>(body: &[Spanned<Statement>], mut pred: F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    any_expr_in_body_impl(body, &mut pred)
}

/// Checks whether a top-level declaration contains any matching expression.
///
/// This helper is responsible for declaration-specific expression slots (for example defaults, decorators, and
/// method/function bodies).
fn declaration_has_expr<F>(decl: &Spanned<Declaration>, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    match &decl.node {
        Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Partial(_)
        | Declaration::TypeAlias(_)
        | Declaration::Docstring(_) => false,
        Declaration::Const(c) => expr_has(&c.value.node, pred),
        Declaration::Static(s) => expr_has(&s.value.node, pred),
        Declaration::Model(m) => {
            decorators_have_expr(&m.decorators, pred)
                || m.fields.iter().any(|field| {
                    field
                        .node
                        .default
                        .as_ref()
                        .is_some_and(|value| expr_has(&value.node, pred))
                })
                || m.methods.iter().any(|method| method_has_expr(&method.node, pred))
        }
        Declaration::Class(c) => {
            decorators_have_expr(&c.decorators, pred)
                || c.fields.iter().any(|field| {
                    field
                        .node
                        .default
                        .as_ref()
                        .is_some_and(|value| expr_has(&value.node, pred))
                })
                || c.methods.iter().any(|method| method_has_expr(&method.node, pred))
        }
        Declaration::Trait(t) => {
            decorators_have_expr(&t.decorators, pred)
                || t.methods.iter().any(|method| method_has_expr(&method.node, pred))
        }
        Declaration::Newtype(n) => {
            decorators_have_expr(&n.decorators, pred)
                || n.rebindings
                    .iter()
                    .any(|rebinding| expr_has(&rebinding.node.target.node, pred))
                || n.interop_edges
                    .iter()
                    .any(|edge| expr_has(&edge.node.adapter.node, pred))
                || n.methods.iter().any(|method| method_has_expr(&method.node, pred))
        }
        Declaration::Enum(e) => decorators_have_expr(&e.decorators, pred),
        Declaration::Function(f) => {
            decorators_have_expr(&f.decorators, pred)
                || params_have_expr(&f.params, pred)
                || any_expr_in_body_impl(&f.body, pred)
        }
        Declaration::TestModule(test_module) => test_module.body.iter().any(|decl| declaration_has_expr(decl, pred)),
    }
}

/// Checks whether a method declaration contains any matching expression.
///
/// Abstract methods (`body: None`) only contribute via decorators/parameter defaults.
fn method_has_expr<F>(method: &crate::frontend::ast::MethodDecl, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    decorators_have_expr(&method.decorators, pred)
        || params_have_expr(&method.params, pred)
        || method
            .body
            .as_ref()
            .is_some_and(|body| any_expr_in_body_impl(body, pred))
}

/// Checks parameter default expressions for a match.
fn params_have_expr<F>(params: &[Spanned<crate::frontend::ast::Param>], pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    params.iter().any(|param| {
        param
            .node
            .default
            .as_ref()
            .is_some_and(|value| expr_has(&value.node, pred))
    })
}

/// Checks decorator argument expressions for a match.
///
/// Type-valued named decorator args are intentionally excluded.
fn decorators_have_expr<F>(decorators: &[Spanned<crate::frontend::ast::Decorator>], pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    decorators.iter().any(|decorator| {
        decorator.node.args.iter().any(|arg| match arg {
            DecoratorArg::Positional(value) => expr_has(&value.node, pred),
            DecoratorArg::Named(_, value) => match value {
                DecoratorArgValue::Type(_) => false,
                DecoratorArgValue::Expr(value) => expr_has(&value.node, pred),
            },
        })
    })
}

/// Checks a statement slice in source order, short-circuiting on first match.
fn any_expr_in_body_impl<F>(body: &[Spanned<Statement>], pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    body.iter().any(|stmt| statement_has_expr(&stmt.node, pred))
}

/// Checks a single statement and all expression-bearing descendants for a match.
fn statement_has_expr<F>(stmt: &Statement, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    // ---- Context: direct expression-bearing statement forms ----
    match stmt {
        Statement::Assignment(a) => expr_has(&a.value.node, pred),
        Statement::FieldAssignment(a) => expr_has(&a.object.node, pred) || expr_has(&a.value.node, pred),
        Statement::IndexAssignment(a) => {
            expr_has(&a.object.node, pred) || expr_has(&a.index.node, pred) || expr_has(&a.value.node, pred)
        }
        Statement::Return(Some(expr)) => expr_has(&expr.node, pred),
        Statement::Expr(expr) => expr_has(&expr.node, pred),
        Statement::CompoundAssignment(a) => expr_has(&a.value.node, pred),
        Statement::TupleUnpack(u) => expr_has(&u.value.node, pred),
        Statement::TupleAssign(a) => {
            a.targets.iter().any(|target| expr_has(&target.node, pred)) || expr_has(&a.value.node, pred)
        }
        Statement::ChainedAssignment(a) => expr_has(&a.value.node, pred),
        Statement::Assert(assert_stmt) => assert_has_expr(assert_stmt, pred),
        Statement::Surface(surface_stmt) => match &surface_stmt.payload {
            crate::frontend::ast::SurfaceStmtPayload::KeywordArgs(args) => {
                args.iter().any(|arg| expr_has(&arg.node, pred))
            }
        },
        Statement::VocabBlock(block) => {
            decorators_have_expr(&block.decorators, pred)
                || block.header_args.iter().any(|arg| expr_has(&arg.node, pred))
                || any_expr_in_body_impl(&block.body, pred)
        }

        // ---- Context: control-flow statements ----
        Statement::If(s) => {
            if condition_has_expr(&s.condition, pred) || any_expr_in_body_impl(&s.then_body, pred) {
                return true;
            }

            for (condition, body) in &s.elif_branches {
                if expr_has(&condition.node, pred) || any_expr_in_body_impl(body, pred) {
                    return true;
                }
            }

            s.else_body
                .as_ref()
                .is_some_and(|else_body| any_expr_in_body_impl(else_body, pred))
        }
        Statement::Loop(s) => any_expr_in_body_impl(&s.body, pred),
        Statement::While(s) => condition_has_expr(&s.condition, pred) || any_expr_in_body_impl(&s.body, pred),
        Statement::For(s) => expr_has(&s.iter.node, pred) || any_expr_in_body_impl(&s.body, pred),
        Statement::Break(Some(expr)) => expr_has(&expr.node, pred),
        Statement::Return(None) | Statement::Break(None) | Statement::Pass | Statement::Continue => false,
    }
}

fn assert_has_expr<F>(assert_stmt: &crate::frontend::ast::AssertStmt, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    let has_core_expr = match &assert_stmt.kind {
        AssertKind::Condition(condition) => expr_has(&condition.node, pred),
        AssertKind::IsPattern { value, .. } => expr_has(&value.node, pred),
        AssertKind::Raises { call, .. } => expr_has(&call.node, pred),
    };
    has_core_expr
        || assert_stmt
            .message
            .as_ref()
            .is_some_and(|message| expr_has(&message.node, pred))
}

fn condition_has_expr<F>(condition: &Condition, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    match condition {
        Condition::Expr(expr) => expr_has(&expr.node, pred),
        Condition::Let { value, .. } => expr_has(&value.node, pred),
    }
}

/// Recursive expression walker used by all traversal entry points.
///
/// Visits `expr` first (pre-order) and then descends into child expressions in source order until `pred` returns
/// `true`.
fn expr_has<F>(expr: &Expr, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    if pred(expr) {
        return true;
    }

    // ---- Context: literal / leaf expressions ----
    match expr {
        Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr => false,

        // ---- Context: unary / binary / call-like expressions ----
        Expr::Binary(left, _, right) | Expr::Index(left, right) => {
            expr_has(&left.node, pred) || expr_has(&right.node, pred)
        }
        Expr::Unary(_, operand) | Expr::Try(operand) | Expr::Paren(operand) | Expr::Field(operand, _) => {
            expr_has(&operand.node, pred)
        }
        Expr::Call(callee, _type_args, args) => {
            expr_has(&callee.node, pred) || args.iter().any(|arg| call_arg_has_expr(arg, pred))
        }
        Expr::MethodCall(base, _, _type_args, args) => {
            expr_has(&base.node, pred) || args.iter().any(|arg| call_arg_has_expr(arg, pred))
        }
        Expr::Constructor(_, args) => args.iter().any(|arg| call_arg_has_expr(arg, pred)),

        // ---- Context: structured / control-flow expressions ----
        Expr::Slice(base, slice) => {
            expr_has(&base.node, pred)
                || slice.start.as_ref().is_some_and(|expr| expr_has(&expr.node, pred))
                || slice.end.as_ref().is_some_and(|expr| expr_has(&expr.node, pred))
                || slice.step.as_ref().is_some_and(|expr| expr_has(&expr.node, pred))
        }
        Expr::Match(scrutinee, arms) => {
            if expr_has(&scrutinee.node, pred) {
                return true;
            }

            for arm in arms {
                if arm.node.guard.as_ref().is_some_and(|guard| expr_has(&guard.node, pred)) {
                    return true;
                }
                match &arm.node.body {
                    MatchBody::Expr(value) => {
                        if expr_has(&value.node, pred) {
                            return true;
                        }
                    }
                    MatchBody::Block(body) => {
                        if any_expr_in_body_impl(body, pred) {
                            return true;
                        }
                    }
                }
            }
            false
        }
        Expr::If(if_expr) => {
            expr_has(&if_expr.condition.node, pred)
                || any_expr_in_body_impl(&if_expr.then_body, pred)
                || if_expr
                    .else_body
                    .as_ref()
                    .is_some_and(|else_body| any_expr_in_body_impl(else_body, pred))
        }
        Expr::Loop(loop_expr) => any_expr_in_body_impl(&loop_expr.body, pred),
        Expr::Generator(generator) => {
            expr_has(&generator.expr.node, pred)
                || generator.clauses.iter().any(|clause| match clause {
                    ComprehensionClause::For { iter, .. } => expr_has(&iter.node, pred),
                    ComprehensionClause::If(condition) => expr_has(&condition.node, pred),
                })
        }
        Expr::ListComp(comp) => {
            expr_has(&comp.expr.node, pred)
                || expr_has(&comp.iter.node, pred)
                || comp.filter.as_ref().is_some_and(|filter| expr_has(&filter.node, pred))
        }
        Expr::DictComp(comp) => {
            expr_has(&comp.key.node, pred)
                || expr_has(&comp.value.node, pred)
                || expr_has(&comp.iter.node, pred)
                || comp.filter.as_ref().is_some_and(|filter| expr_has(&filter.node, pred))
        }
        Expr::Closure(params, body) => params_have_expr(params, pred) || expr_has(&body.node, pred),
        Expr::Range { start, end, .. } => expr_has(&start.node, pred) || expr_has(&end.node, pred),
        Expr::Surface(surface_expr) => match &surface_expr.payload {
            crate::frontend::ast::SurfaceExprPayload::PrefixUnary(expr) => expr_has(&expr.node, pred),
            crate::frontend::ast::SurfaceExprPayload::LeadingDotPath { .. } => false,
            crate::frontend::ast::SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                expr_has(&left.node, pred) || expr_has(&right.node, pred)
            }
            crate::frontend::ast::SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                args.iter().any(|arg| match arg {
                    crate::frontend::ast::CallArg::Positional(expr)
                    | crate::frontend::ast::CallArg::Named(_, expr)
                    | crate::frontend::ast::CallArg::PositionalUnpack(expr)
                    | crate::frontend::ast::CallArg::KeywordUnpack(expr) => expr_has(&expr.node, pred),
                })
            }
        },

        // ---- Context: collection / interpolation expressions ----
        Expr::Tuple(items) | Expr::Set(items) => items.iter().any(|item| expr_has(&item.node, pred)),
        Expr::List(entries) => entries.iter().any(|entry| match entry {
            ListEntry::Element(value) | ListEntry::Spread(value) => expr_has(&value.node, pred),
        }),
        Expr::Dict(entries) => entries.iter().any(|entry| match entry {
            DictEntry::Pair(key, value) => expr_has(&key.node, pred) || expr_has(&value.node, pred),
            DictEntry::Spread(value) => expr_has(&value.node, pred),
        }),
        Expr::FString(parts) => parts.iter().any(|part| match part {
            crate::frontend::ast::FStringPart::Literal(_) => false,
            crate::frontend::ast::FStringPart::Expr(expr) => expr_has(&expr.node, pred),
        }),
        Expr::Yield(Some(expr)) => expr_has(&expr.node, pred),
        Expr::Yield(None) | Expr::Partial(_) => false,
    }
}

/// Checks a call argument expression for a match.
fn call_arg_has_expr<F>(arg: &CallArg, pred: &mut F) -> bool
where
    F: FnMut(&Expr) -> bool,
{
    match arg {
        CallArg::Positional(expr)
        | CallArg::Named(_, expr)
        | CallArg::PositionalUnpack(expr)
        | CallArg::KeywordUnpack(expr) => expr_has(&expr.node, pred),
    }
}
