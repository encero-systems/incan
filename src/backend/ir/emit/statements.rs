//! Statement emission for IR to Rust code generation
//!
//! This module handles emitting Rust statements from IR statements,
//! including let bindings, assignments, control flow, and blocks.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;

use incan_core::lang::stdlib;

use super::super::expr::{
    BuiltinFn, IrCallArgKind, IrDictEntry, IrExprKind, IrGeneratorClause, IrListEntry, MatchArm, Pattern, TypedExpr,
    VarAccess,
};
use super::super::ownership::{LoopIterationPlan, ValueUseSite, plan_for_loop_iteration, plan_value_use};
use super::super::scanners::{binding_use_scan, expr_uses_binding_name};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::super::types::Mutability;
use super::{EmitError, IrEmitter};
use crate::backend::ir::emit::expressions::{
    method_dispatch_uses_mutable_receiver, method_kind_uses_mutable_receiver, method_name_uses_mutable_receiver,
};

/// Get the root variable name of an expression.
fn root_var_name(expr: &super::super::expr::IrExpr) -> Option<&str> {
    match &expr.kind {
        IrExprKind::Var { name, .. } => Some(name.as_str()),
        IrExprKind::Field { object, .. } => root_var_name(object),
        IrExprKind::Index { object, .. } => root_var_name(object),
        _ => None,
    }
}

/// Check if an assignment target mutates a variable.
fn target_mutates_var(target: &AssignTarget, var: &str) -> bool {
    match target {
        AssignTarget::Var(name) => name == var,
        AssignTarget::StaticBinding(name) => name == var,
        AssignTarget::Static(_) => false,
        AssignTarget::Field { object, .. } => root_var_name(object).is_some_and(|n| n == var),
        AssignTarget::Index { object, .. } => root_var_name(object).is_some_and(|n| n == var),
    }
}

/// Check if an expression contains a mutation of a variable.
fn expr_contains_mutation(expr: &super::super::expr::IrExpr, var: &str) -> bool {
    match &expr.kind {
        IrExprKind::Var {
            name,
            access: VarAccess::BorrowMut,
            ..
        } => name == var,
        IrExprKind::BinOp { left, right, .. } => {
            expr_contains_mutation(left, var) || expr_contains_mutation(right, var)
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => expr_contains_mutation(operand, var),
        IrExprKind::Call { func, args, .. } => {
            expr_contains_mutation(func, var) || args.iter().any(|arg| expr_contains_mutation(&arg.expr, var))
        }
        IrExprKind::BuiltinCall { args, .. } => args.iter().any(|arg| expr_contains_mutation(arg, var)),
        IrExprKind::MethodCall {
            receiver,
            method,
            args,
            arg_policy,
            dispatch,
            ..
        } => {
            ((!matches!(arg_policy, super::super::expr::MethodCallArgPolicy::PreserveShape)
                && method_name_uses_mutable_receiver(method)
                || method_dispatch_uses_mutable_receiver(dispatch.as_ref()))
                && root_var_name(receiver).is_some_and(|name| name == var))
                || expr_contains_mutation(receiver, var)
                || args.iter().any(|arg| expr_contains_mutation(&arg.expr, var))
        }
        IrExprKind::KnownMethodCall { receiver, kind, args } => {
            (method_kind_uses_mutable_receiver(kind) && root_var_name(receiver).is_some_and(|name| name == var))
                || expr_contains_mutation(receiver, var)
                || args.iter().any(|arg| expr_contains_mutation(&arg.expr, var))
        }
        IrExprKind::Field { object, .. } => expr_contains_mutation(object, var),
        IrExprKind::Index { object, index } => {
            expr_contains_mutation(object, var) || expr_contains_mutation(index, var)
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            expr_contains_mutation(target, var)
                || start.as_ref().is_some_and(|value| expr_contains_mutation(value, var))
                || end.as_ref().is_some_and(|value| expr_contains_mutation(value, var))
                || step.as_ref().is_some_and(|value| expr_contains_mutation(value, var))
        }
        IrExprKind::Set(items) | IrExprKind::Tuple(items) => items.iter().any(|item| expr_contains_mutation(item, var)),
        IrExprKind::List(items) => items.iter().any(|item| match item {
            IrListEntry::Element(value) | IrListEntry::Spread(value) => expr_contains_mutation(value, var),
        }),
        IrExprKind::Dict(entries) => entries.iter().any(|entry| match entry {
            IrDictEntry::Pair(key, value) => expr_contains_mutation(key, var) || expr_contains_mutation(value, var),
            IrDictEntry::Spread(value) => expr_contains_mutation(value, var),
        }),
        IrExprKind::Struct { fields, .. } => fields.iter().any(|(_, value)| expr_contains_mutation(value, var)),
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_contains_mutation(condition, var)
                || expr_contains_mutation(then_branch, var)
                || else_branch
                    .as_ref()
                    .is_some_and(|else_branch| expr_contains_mutation(else_branch, var))
        }
        IrExprKind::Block { stmts, value } => {
            stmts.iter().any(|s| stmt_mutates_var(s, var))
                || value.as_ref().is_some_and(|v| expr_contains_mutation(v, var))
        }
        IrExprKind::Loop { body } => body.iter().any(|stmt| stmt_mutates_var(stmt, var)),
        IrExprKind::Race { arms, .. } => arms
            .iter()
            .any(|arm| expr_contains_mutation(&arm.awaitable, var) || expr_contains_mutation(&arm.body, var)),
        IrExprKind::Match { scrutinee, arms } => {
            expr_contains_mutation(scrutinee, var)
                || arms.iter().any(|arm| {
                    arm.bindings.iter().any(|binding| {
                        expr_contains_mutation(&binding.value, var)
                            || binding
                                .guard_value
                                .as_ref()
                                .is_some_and(|guard_value| expr_contains_mutation(guard_value, var))
                    }) || arm
                        .guard
                        .as_ref()
                        .is_some_and(|guard| expr_contains_mutation(guard, var))
                        || expr_contains_mutation(&arm.body, var)
                })
        }
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            expr_contains_mutation(element, var)
                || expr_contains_mutation(iterable, var)
                || filter
                    .as_ref()
                    .is_some_and(|filter| expr_contains_mutation(filter, var))
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            expr_contains_mutation(key, var)
                || expr_contains_mutation(value, var)
                || expr_contains_mutation(iterable, var)
                || filter
                    .as_ref()
                    .is_some_and(|filter| expr_contains_mutation(filter, var))
        }
        IrExprKind::Generator { element, clauses } => {
            expr_contains_mutation(element, var)
                || clauses.iter().any(|clause| match clause {
                    IrGeneratorClause::For { iterable, .. } => expr_contains_mutation(iterable, var),
                    IrGeneratorClause::If(condition) => expr_contains_mutation(condition, var),
                })
        }
        IrExprKind::Closure { body, .. } => expr_contains_mutation(body, var),
        IrExprKind::Range { start, end, .. } => {
            start.as_ref().is_some_and(|value| expr_contains_mutation(value, var))
                || end.as_ref().is_some_and(|value| expr_contains_mutation(value, var))
        }
        IrExprKind::Format { parts } => parts.iter().any(|part| match part {
            super::super::expr::FormatPart::Literal(_) => false,
            super::super::expr::FormatPart::Expr { expr, .. } => expr_contains_mutation(expr, var),
        }),
        _ => false,
    }
}

/// Check if a statement mutates a variable.
pub(in crate::backend::ir::emit) fn stmt_mutates_var(stmt: &IrStmt, var: &str) -> bool {
    match &stmt.kind {
        IrStmtKind::Let { value, .. } => expr_contains_mutation(value, var),
        IrStmtKind::Assign { target, value } | IrStmtKind::CompoundAssign { target, value, .. } => {
            target_mutates_var(target, var) || expr_contains_mutation(value, var)
        }
        IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) | IrStmtKind::Yield(expr) => {
            expr_contains_mutation(expr, var)
        }
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_contains_mutation(condition, var)
                || then_branch.iter().any(|s| stmt_mutates_var(s, var))
                || else_branch
                    .as_ref()
                    .is_some_and(|b| b.iter().any(|s| stmt_mutates_var(s, var)))
        }
        IrStmtKind::While { condition, body, .. } => {
            expr_contains_mutation(condition, var) || body.iter().any(|s| stmt_mutates_var(s, var))
        }
        IrStmtKind::For { iterable, body, .. } => {
            expr_contains_mutation(iterable, var) || body.iter().any(|s| stmt_mutates_var(s, var))
        }
        IrStmtKind::Loop { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
        IrStmtKind::Block(stmts) => stmts.iter().any(|s| stmt_mutates_var(s, var)),
        IrStmtKind::Match { scrutinee, arms } => {
            expr_contains_mutation(scrutinee, var)
                || arms.iter().any(|arm| {
                    arm.bindings.iter().any(|binding| {
                        expr_contains_mutation(&binding.value, var)
                            || binding
                                .guard_value
                                .as_ref()
                                .is_some_and(|guard_value| expr_contains_mutation(guard_value, var))
                    }) || arm
                        .guard
                        .as_ref()
                        .is_some_and(|guard| expr_contains_mutation(guard, var))
                        || expr_contains_mutation(&arm.body, var)
                })
        }
        IrStmtKind::Break { label: _, value } => value.as_ref().is_some_and(|value| expr_contains_mutation(value, var)),
        IrStmtKind::Return(None) | IrStmtKind::Continue(_) => false,
    }
}

/// Determine whether a `for` loop body requires mutable iteration of the loop variable.
///
/// We use this as a *codegen heuristic* to avoid emitting `.iter_mut()` when the loop body performs no mutation of the
/// loop item. Emitting `.iter_mut()`:
///
/// - requires mutable access to the source collection, and
/// - changes the loop item type from `&T` to `&mut T`.
fn for_body_needs_mut_iteration(pattern: &Pattern, body: &[IrStmt]) -> bool {
    let loop_var = match pattern {
        Pattern::Var(name) => name.as_str(),
        _ => return false,
    };

    body.iter().any(|s| stmt_mutates_var(s, loop_var))
}

/// Return whether the expression calls a Rust helper that returns `!`.
///
/// Source stdlib code often writes `return raise_value_error(...)` because the source language does not expose Rust's
/// never type. Rust warns on `return <never-expr>`, so emission renders the diverging call directly.
fn is_diverging_rust_error_call(expr: &TypedExpr) -> bool {
    let IrExprKind::Call {
        func, canonical_path, ..
    } = &expr.kind
    else {
        return false;
    };
    let function_name = match &func.kind {
        IrExprKind::Var { name, .. } | IrExprKind::FunctionItem { name, .. } => Some(name.as_str()),
        _ => None,
    };
    if function_name.is_some_and(stdlib::is_diverging_rust_error_helper_name) {
        return true;
    }

    let Some(path) = canonical_path else {
        return false;
    };
    path.len() == 4
        && path[0] == stdlib::STDLIB_RUST
        && path[1] == stdlib::INCAN_STD_NAMESPACE
        && path[2] == stdlib::INCAN_STD_ERRORS_MODULE
        && stdlib::is_diverging_rust_error_helper_name(&path[3])
}

/// Return the element target type for assignment into a list index.
fn list_index_assignment_element_type(object_ty: &IrType) -> Option<&IrType> {
    match object_ty {
        IrType::Ref(inner) | IrType::RefMut(inner) => list_index_assignment_element_type(inner),
        IrType::List(elem_ty) => Some(elem_ty.as_ref()),
        _ => None,
    }
}

/// Return the local `StaticBinding` name at the root of a storage-rooted expression.
///
/// This is used by statement-slice analysis to detect aliases like `live` in
/// `live.append(...)` or `live[i] = ...` so emission can decide whether the local
/// Rust binding must be declared `mut`.
fn expr_storage_binding_root_name(expr: &super::super::expr::IrExpr) -> Option<&str> {
    match &expr.kind {
        IrExprKind::Var {
            name,
            ref_kind: super::super::expr::VarRefKind::StaticBinding,
            ..
        } => Some(name.as_str()),
        IrExprKind::Field { object, .. } | IrExprKind::Index { object, .. } => expr_storage_binding_root_name(object),
        _ => None,
    }
}

/// Collect `StaticBinding` locals whose receiver position implies mutation within one expression tree.
///
/// This walk is intentionally conservative: if an expression path can lower to
/// `binding.with_mut(...)`, the binding name is recorded so the enclosing statement slice
/// can emit `let mut binding = ...` even when the source-level binding itself is not declared
/// `mut`.
fn expr_mutates_storage_binding(expr: &super::super::expr::IrExpr, names: &mut HashSet<String>) {
    // ---- Context: direct receiver mutations from method-call forms ----
    match &expr.kind {
        IrExprKind::MethodCall {
            receiver,
            args,
            arg_policy,
            ..
        } => {
            if !matches!(arg_policy, super::super::expr::MethodCallArgPolicy::PreserveShape)
                && let Some(name) = expr_storage_binding_root_name(receiver)
            {
                names.insert(name.to_string());
            }
            expr_mutates_storage_binding(receiver, names);
            for arg in args {
                expr_mutates_storage_binding(&arg.expr, names);
            }
        }
        IrExprKind::KnownMethodCall { receiver, kind, args } => {
            if method_kind_uses_mutable_receiver(kind)
                && let Some(name) = expr_storage_binding_root_name(receiver)
            {
                names.insert(name.to_string());
            }
            expr_mutates_storage_binding(receiver, names);
            for arg in args {
                expr_mutates_storage_binding(&arg.expr, names);
            }
        }
        // ---- Context: recurse into nested expression trees ----
        IrExprKind::Block { stmts, value } => {
            for stmt in stmts {
                stmt_mutates_storage_binding(stmt, names);
            }
            if let Some(value) = value {
                expr_mutates_storage_binding(value, names);
            }
        }
        IrExprKind::Race { arms, .. } => {
            for arm in arms {
                expr_mutates_storage_binding(&arm.awaitable, names);
                expr_mutates_storage_binding(&arm.body, names);
            }
        }
        IrExprKind::Call { func, args, .. } => {
            expr_mutates_storage_binding(func, names);
            for arg in args {
                expr_mutates_storage_binding(&arg.expr, names);
            }
        }
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                expr_mutates_storage_binding(arg, names);
            }
        }
        IrExprKind::BinOp { left, right, .. } => {
            expr_mutates_storage_binding(left, names);
            expr_mutates_storage_binding(right, names);
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => expr_mutates_storage_binding(operand, names),
        IrExprKind::Field { object, .. } => expr_mutates_storage_binding(object, names),
        IrExprKind::Index { object, index } => {
            expr_mutates_storage_binding(object, names);
            expr_mutates_storage_binding(index, names);
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            expr_mutates_storage_binding(target, names);
            if let Some(start) = start {
                expr_mutates_storage_binding(start, names);
            }
            if let Some(end) = end {
                expr_mutates_storage_binding(end, names);
            }
            if let Some(step) = step {
                expr_mutates_storage_binding(step, names);
            }
        }
        IrExprKind::Set(items) | IrExprKind::Tuple(items) => {
            for item in items {
                expr_mutates_storage_binding(item, names);
            }
        }
        IrExprKind::List(items) => {
            for item in items {
                match item {
                    IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                        expr_mutates_storage_binding(value, names);
                    }
                }
            }
        }
        IrExprKind::Dict(pairs) => {
            for entry in pairs {
                match entry {
                    IrDictEntry::Pair(key, value) => {
                        expr_mutates_storage_binding(key, names);
                        expr_mutates_storage_binding(value, names);
                    }
                    IrDictEntry::Spread(value) => expr_mutates_storage_binding(value, names),
                }
            }
        }
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                expr_mutates_storage_binding(value, names);
            }
        }
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_mutates_storage_binding(condition, names);
            expr_mutates_storage_binding(then_branch, names);
            if let Some(else_branch) = else_branch {
                expr_mutates_storage_binding(else_branch, names);
            }
        }
        IrExprKind::Match { scrutinee, arms } => {
            expr_mutates_storage_binding(scrutinee, names);
            for arm in arms {
                for binding in &arm.bindings {
                    expr_mutates_storage_binding(&binding.value, names);
                    if let Some(guard_value) = &binding.guard_value {
                        expr_mutates_storage_binding(guard_value, names);
                    }
                }
                if let Some(guard) = &arm.guard {
                    expr_mutates_storage_binding(guard, names);
                }
                expr_mutates_storage_binding(&arm.body, names);
            }
        }
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            expr_mutates_storage_binding(element, names);
            expr_mutates_storage_binding(iterable, names);
            if let Some(filter) = filter {
                expr_mutates_storage_binding(filter, names);
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            expr_mutates_storage_binding(key, names);
            expr_mutates_storage_binding(value, names);
            expr_mutates_storage_binding(iterable, names);
            if let Some(filter) = filter {
                expr_mutates_storage_binding(filter, names);
            }
        }
        IrExprKind::Generator { element, clauses } => {
            expr_mutates_storage_binding(element, names);
            for clause in clauses {
                match clause {
                    IrGeneratorClause::For { iterable, .. } => expr_mutates_storage_binding(iterable, names),
                    IrGeneratorClause::If(condition) => expr_mutates_storage_binding(condition, names),
                }
            }
        }
        IrExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                expr_mutates_storage_binding(start, names);
            }
            if let Some(end) = end {
                expr_mutates_storage_binding(end, names);
            }
        }
        // ---- Context: leaf expressions have no nested mutation path ----
        _ => {}
    }
}

/// Collect `StaticBinding` locals whose values are mutated anywhere inside one statement.
///
/// The resulting names feed statement-slice emission so only storage aliases that truly need
/// mutable Rust handles are emitted with `let mut`.
fn stmt_mutates_storage_binding(stmt: &IrStmt, names: &mut HashSet<String>) {
    match &stmt.kind {
        // ---- Context: single-expression statement forms ----
        IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) | IrStmtKind::Yield(expr) => {
            expr_mutates_storage_binding(expr, names);
        }
        IrStmtKind::Let { value, .. } => expr_mutates_storage_binding(value, names),
        IrStmtKind::Assign { target, value } => {
            match target {
                AssignTarget::StaticBinding(name) => {
                    names.insert(name.clone());
                }
                AssignTarget::Field { object, .. } | AssignTarget::Index { object, .. } => {
                    if let Some(name) = expr_storage_binding_root_name(object) {
                        names.insert(name.to_string());
                    }
                }
                AssignTarget::Var(_) | AssignTarget::Static(_) => {}
            }
            expr_mutates_storage_binding(value, names);
        }
        // ---- Context: recurse into control-flow bodies ----
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_mutates_storage_binding(condition, names);
            for stmt in then_branch {
                stmt_mutates_storage_binding(stmt, names);
            }
            if let Some(else_branch) = else_branch {
                for stmt in else_branch {
                    stmt_mutates_storage_binding(stmt, names);
                }
            }
        }
        IrStmtKind::While { condition, body, .. } => {
            expr_mutates_storage_binding(condition, names);
            for stmt in body {
                stmt_mutates_storage_binding(stmt, names);
            }
        }
        IrStmtKind::For { iterable, body, .. } => {
            expr_mutates_storage_binding(iterable, names);
            for stmt in body {
                stmt_mutates_storage_binding(stmt, names);
            }
        }
        IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => {
            for stmt in body {
                stmt_mutates_storage_binding(stmt, names);
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            expr_mutates_storage_binding(scrutinee, names);
            for arm in arms {
                for binding in &arm.bindings {
                    expr_mutates_storage_binding(&binding.value, names);
                    if let Some(guard_value) = &binding.guard_value {
                        expr_mutates_storage_binding(guard_value, names);
                    }
                }
                if let Some(guard) = &arm.guard {
                    expr_mutates_storage_binding(guard, names);
                }
                expr_mutates_storage_binding(&arm.body, names);
            }
        }
        // ---- Context: terminal/unsupported statement kinds ----
        IrStmtKind::Return(None)
        | IrStmtKind::Break { label: _, value: _ }
        | IrStmtKind::Continue(_)
        | IrStmtKind::CompoundAssign { .. } => {}
    }
}

/// Compute the set of storage-backed local aliases that require mutable Rust bindings.
///
/// This is a pre-pass over a statement slice. It preserves warning-free read-only aliases while keeping
/// mutation-capable aliases compilable.
fn collect_mutated_storage_bindings(stmts: &[IrStmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in stmts {
        stmt_mutates_storage_binding(stmt, &mut names);
    }
    names
}

/// Compute per-statement usage for local `let` bindings in one sibling slice.
///
/// Each `let` is considered used only when later code can still resolve that name to the same binding. A subsequent
/// same-name `let` shadows it, and inner scopes handle their own shadowing before scanning nested bodies.
fn collect_local_binding_usage(
    stmts: &[IrStmt],
    following_slices: &[&[IrStmt]],
    following_expr: Option<&TypedExpr>,
) -> Vec<Option<bool>> {
    stmts
        .iter()
        .enumerate()
        .map(|(index, stmt)| match &stmt.kind {
            IrStmtKind::Let { name, .. } => {
                Some(binding_use_scan(&stmts[index + 1..], following_slices, following_expr, name).used)
            }
            _ => None,
        })
        .collect()
}

/// Return whether an IR statement definitely exits the current statement slice.
fn stmt_always_diverges(stmt: &IrStmt) -> bool {
    match &stmt.kind {
        IrStmtKind::Return(_) | IrStmtKind::Break { .. } | IrStmtKind::Continue(_) => true,
        IrStmtKind::Block(stmts) => stmts.last().is_some_and(stmt_always_diverges),
        IrStmtKind::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => {
            then_branch.last().is_some_and(stmt_always_diverges) && else_branch.last().is_some_and(stmt_always_diverges)
        }
        IrStmtKind::Match { scrutinee, arms } => {
            match_arms_are_exhaustive(&scrutinee.ty, arms)
                && arms
                    .iter()
                    .all(|arm| arm.guard.is_none() && expr_always_diverges(&arm.body))
        }
        _ => false,
    }
}

/// Return whether an IR expression definitely exits the current statement slice.
fn expr_always_diverges(expr: &TypedExpr) -> bool {
    match &expr.kind {
        IrExprKind::Block { stmts, value } => {
            stmts.last().is_some_and(stmt_always_diverges) || value.as_deref().is_some_and(expr_always_diverges)
        }
        IrExprKind::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => expr_always_diverges(then_branch) && expr_always_diverges(else_branch),
        IrExprKind::Match { scrutinee, arms } => {
            match_arms_are_exhaustive(&scrutinee.ty, arms)
                && arms
                    .iter()
                    .all(|arm| arm.guard.is_none() && expr_always_diverges(&arm.body))
        }
        _ => false,
    }
}

/// Return whether a match arm set covers the currently known scrutinee shape.
fn match_arms_are_exhaustive(scrutinee_ty: &IrType, arms: &[MatchArm]) -> bool {
    if arms.is_empty() {
        return false;
    }
    if arms
        .iter()
        .any(|arm| arm.guard.is_none() && matches!(arm.pattern, Pattern::Wildcard | Pattern::Var(_)))
    {
        return true;
    }
    let Some(union_name) = scrutinee_ty.union_type_name() else {
        return false;
    };
    let Some(members) = scrutinee_ty.union_members() else {
        return false;
    };
    let mut covered = HashSet::new();
    for arm in arms {
        if arm.guard.is_some() {
            continue;
        }
        collect_union_variant_patterns(&arm.pattern, &union_name, &mut covered);
    }
    covered.len() == members.len()
}

/// Collect anonymous-union variant names covered by one pattern.
fn collect_union_variant_patterns(pattern: &Pattern, union_name: &str, covered: &mut HashSet<String>) {
    match pattern {
        Pattern::Enum { variant, .. } => {
            if let Some((name, variant)) = variant.split_once("::")
                && name == union_name
            {
                covered.insert(variant.to_string());
            }
        }
        Pattern::Or(patterns) => {
            for pattern in patterns {
                collect_union_variant_patterns(pattern, union_name, covered);
            }
        }
        _ => {}
    }
}

/// Replace pattern captures that are not read by the arm body, guard, or compiler-inserted bindings with wildcards.
fn erase_unused_pattern_bindings(pattern: &Pattern, arm: &MatchArm) -> Pattern {
    match pattern {
        Pattern::Var(name) if !arm_uses_pattern_binding(arm, name) => Pattern::Wildcard,
        Pattern::Var(_) | Pattern::Wildcard | Pattern::Literal(_) => pattern.clone(),
        Pattern::Tuple(items) => Pattern::Tuple(
            items
                .iter()
                .map(|item| erase_unused_pattern_bindings(item, arm))
                .collect(),
        ),
        Pattern::Struct { name, fields } => Pattern::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, pattern)| (field.clone(), erase_unused_pattern_bindings(pattern, arm)))
                .collect(),
        },
        Pattern::Enum { name, variant, fields } => Pattern::Enum {
            name: name.clone(),
            variant: variant.clone(),
            fields: fields
                .iter()
                .map(|field| erase_unused_pattern_bindings(field, arm))
                .collect(),
        },
        Pattern::Or(items) => Pattern::Or(
            items
                .iter()
                .map(|item| erase_unused_pattern_bindings(item, arm))
                .collect(),
        ),
    }
}

/// Return whether one pattern binding is used after pattern matching.
fn arm_uses_pattern_binding(arm: &MatchArm, name: &str) -> bool {
    arm.guard
        .as_ref()
        .is_some_and(|guard| expr_uses_binding_name(guard, name))
        || expr_uses_binding_name(&arm.body, name)
        || arm.bindings.iter().any(|binding| {
            expr_uses_binding_name(&binding.value, name)
                || binding
                    .guard_value
                    .as_ref()
                    .is_some_and(|guard_value| expr_uses_binding_name(guard_value, name))
        })
}

impl<'a> IrEmitter<'a> {
    /// Emit a sibling statement slice with precomputed binding context.
    ///
    /// Storage-alias mutability is tracked as a frame because helper paths may emit nested statements. Plain local
    /// usage is indexed by sibling position so same-name shadowing does not keep unused bindings warning-prone.
    pub(super) fn emit_stmts(&self, stmts: &[IrStmt]) -> Result<Vec<TokenStream>, EmitError> {
        self.emit_stmts_with_tail(stmts, &[], None)
    }

    /// Emit statements that precede a final expression in the same Rust block.
    pub(super) fn emit_stmts_before_expr(
        &self,
        stmts: &[IrStmt],
        value: &TypedExpr,
    ) -> Result<Vec<TokenStream>, EmitError> {
        self.emit_stmts_with_tail(stmts, &[], Some(value))
    }

    /// Emit statements with extra same-scope usage tails from lowered block-expression statements.
    fn emit_stmts_with_tail(
        &self,
        stmts: &[IrStmt],
        following_slices: &[&[IrStmt]],
        following_expr: Option<&TypedExpr>,
    ) -> Result<Vec<TokenStream>, EmitError> {
        let mutated = collect_mutated_storage_bindings(stmts);
        let local_usage = collect_local_binding_usage(stmts, following_slices, following_expr);
        self.storage_binding_mut_names.borrow_mut().push(mutated);
        let emitted = (|| {
            let mut emitted = Vec::new();
            for (index, stmt) in stmts.iter().enumerate() {
                let mut next_slices = Vec::with_capacity(following_slices.len() + 1);
                next_slices.push(&stmts[index + 1..]);
                next_slices.extend_from_slice(following_slices);
                emitted.push(self.emit_stmt_with_local_usage(
                    stmt,
                    local_usage[index],
                    &next_slices,
                    following_expr,
                )?);
                if stmt_always_diverges(stmt) {
                    break;
                }
            }
            Ok(emitted)
        })();
        self.storage_binding_mut_names.borrow_mut().pop();
        emitted
    }

    /// Check whether the current statement-slice context requires `name` to be emitted as `let mut`.
    ///
    /// This is only used for local aliases created from `IrExprKind::StaticBinding`.
    fn current_storage_binding_needs_mut(&self, name: &str) -> bool {
        self.storage_binding_mut_names
            .borrow()
            .iter()
            .rev()
            .any(|names| names.contains(name))
    }

    /// Emit assignment to a local `StaticBinding` variable.
    ///
    /// Plain values are wrapped into `StaticBinding::from_value(...)` so subsequent storage-aware field/index
    /// operations can treat the binding uniformly as a storage handle.
    fn emit_static_binding_assignment(
        &self,
        name: &str,
        value: &super::super::expr::IrExpr,
    ) -> Result<TokenStream, EmitError> {
        let n = Self::rust_ident(name);
        let v = if matches!(value.kind, IrExprKind::StaticBinding { .. }) {
            self.emit_assignment_value(value, None)?
        } else {
            let emitted = self.emit_assignment_value(value, None)?;
            quote! { incan_stdlib::storage::StaticBinding::from_value((#emitted).into()) }
        };
        Ok(quote! { #n = #v; })
    }

    /// Emit an assignment RHS, seeding `Result` constructors from the assignment type when possible.
    ///
    /// Assignment-like contexts can carry enough type information to stabilize `Ok`/`Err` emission even when plain
    /// expression emission would leave Rust inference underconstrained.
    fn emit_assignment_value(&self, value: &TypedExpr, expected_ty: Option<&IrType>) -> Result<TokenStream, EmitError> {
        if let Some(target_ty) = expected_ty
            && let Some(wrapped) = self.emit_union_wrapped_value(value, target_ty, false)?
        {
            return Ok(wrapped);
        }
        if let Some(target_ty) = expected_ty
            && self.union_widening_needed(&value.ty, target_ty)
        {
            return self.emit_expr_for_use(
                value,
                ValueUseSite::Assignment {
                    target_ty: Some(target_ty),
                },
            );
        }

        if let Some(target_ty) = expected_ty
            && let Some(seed) = self.emit_inference_seeded_literal_arg(value, target_ty)?
        {
            return Ok(seed);
        }

        let can_seed_result_constructor = matches!(value.kind, IrExprKind::Call { .. } | IrExprKind::Struct { .. });

        if can_seed_result_constructor {
            if let Some(target_ty) = expected_ty
                && matches!(target_ty, IrType::Result(_, _))
                && let Some(seed) = self.emit_inference_seeded_literal_arg(value, target_ty)?
            {
                return Ok(seed);
            }
            if matches!(&value.ty, IrType::Result(_, _))
                && let Some(seed) = self.emit_inference_seeded_literal_arg(value, &value.ty)?
            {
                return Ok(seed);
            }
        }
        let call_like_value = match &value.kind {
            IrExprKind::Call { .. } | IrExprKind::MethodCall { .. } | IrExprKind::Try(_) => true,
            IrExprKind::InteropCoerce { expr, .. } => {
                matches!(
                    expr.kind,
                    IrExprKind::Call { .. } | IrExprKind::MethodCall { .. } | IrExprKind::Try(_)
                )
            }
            IrExprKind::Cast { expr, .. } | IrExprKind::Await(expr) => {
                matches!(
                    expr.kind,
                    IrExprKind::Call { .. } | IrExprKind::MethodCall { .. } | IrExprKind::Try(_)
                )
            }
            _ => false,
        };
        if let Some(target_ty) = expected_ty
            && call_like_value
        {
            return self.emit_expr_for_use(
                value,
                ValueUseSite::Assignment {
                    target_ty: Some(target_ty),
                },
            );
        }
        self.emit_expr(value)
    }

    /// Emit a concrete member value wrapped in the generated union variant required by the target type.
    fn emit_union_wrapped_value(
        &self,
        value: &TypedExpr,
        target_ty: &IrType,
        in_return: bool,
    ) -> Result<Option<TokenStream>, EmitError> {
        if value.ty.is_union() {
            return Ok(None);
        }
        let Some(variant_index) = target_ty.union_variant_index_for_member(&value.ty) else {
            return Ok(None);
        };
        let Some(members) = target_ty.union_members() else {
            return Ok(None);
        };
        let Some(member_ty) = members.get(variant_index) else {
            return Ok(None);
        };
        let variant_ident = format_ident!("{}", IrType::union_variant_name(variant_index));
        let union_path = self.emit_union_type_path(target_ty);
        let emitted = if in_return {
            self.emit_expr_for_use(
                value,
                ValueUseSite::ReturnValue {
                    target_ty: Some(member_ty),
                },
            )?
        } else {
            self.emit_expr_for_use(
                value,
                ValueUseSite::Assignment {
                    target_ty: Some(member_ty),
                },
            )?
        };
        Ok(Some(quote! { #union_path :: #variant_ident(#emitted) }))
    }

    /// Return a Rust local type annotation for explicit Incan bindings that can be named in local position.
    fn emit_local_let_annotation(&self, ty: &IrType) -> Option<TokenStream> {
        match ty {
            IrType::Unknown | IrType::Trait(_) | IrType::ImplTrait(_) => None,
            _ => {
                let ty_tokens = self.emit_type(ty);
                Some(quote! { : #ty_tokens })
            }
        }
    }

    /// Emit assignment through a storage-rooted field or index path.
    ///
    /// This rewrites the target to use the `with_mut` temporary binding and evaluates the RHS once before entering the
    /// mutation closure.
    fn emit_storage_rooted_assignment(
        &self,
        target: &AssignTarget,
        value: &super::super::expr::IrExpr,
    ) -> Result<TokenStream, EmitError> {
        let local_name = "__incan_static_value";
        let rhs_name = "__incan_static_rhs";
        let rhs_ident = format_ident!("{}", rhs_name);
        let rewritten_target = match target {
            AssignTarget::Field { object, field } => AssignTarget::Field {
                object: Box::new(Self::rewrite_storage_root_expr_for_mut(object, local_name)),
                field: field.clone(),
            },
            AssignTarget::Index { object, index } => AssignTarget::Index {
                object: Box::new(Self::rewrite_storage_root_expr_for_mut(object, local_name)),
                index: index.clone(),
            },
            _ => {
                return Err(EmitError::Unsupported(
                    "expected field or index assignment for storage-rooted target".to_string(),
                ));
            }
        };
        let rhs_expr = super::super::expr::TypedExpr::new(
            IrExprKind::Var {
                name: rhs_name.to_string(),
                access: super::super::expr::VarAccess::Move,
                ref_kind: super::super::expr::VarRefKind::Value,
            },
            value.ty.clone(),
        );
        let inner_stmt = IrStmt::new(IrStmtKind::Assign {
            target: rewritten_target,
            value: rhs_expr,
        });
        let inner = self.emit_stmt(&inner_stmt)?;
        let storage_expr = match target {
            AssignTarget::Field { object, .. } | AssignTarget::Index { object, .. } => object.as_ref(),
            _ => unreachable!("guarded above"),
        };
        let emitted_value = self.emit_assignment_value(value, None)?;
        let wrapped = self.emit_storage_with_mut(storage_expr, inner)?;
        Ok(quote! {
            let #rhs_ident = #emitted_value;
            #wrapped
        })
    }

    /// Emit a statement as Rust tokens.
    pub(super) fn emit_stmt(&self, stmt: &IrStmt) -> Result<TokenStream, EmitError> {
        self.emit_stmt_with_local_usage(stmt, None, &[], None)
    }

    /// Emit a statement with optional sibling-slice local usage context.
    fn emit_stmt_with_local_usage(
        &self,
        stmt: &IrStmt,
        local_binding_is_used: Option<bool>,
        following_slices: &[&[IrStmt]],
        following_expr: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        match &stmt.kind {
            IrStmtKind::Expr(expr) => {
                // Lowering currently models tuple-unpack/chained-assignment expansion as a block
                // expression used in statement position. Emit those inner statements directly so
                // the introduced bindings remain visible to following statements.
                if let IrExprKind::Block { stmts, value: None } = &expr.kind {
                    let inner = self.emit_stmts_with_tail(stmts, following_slices, following_expr)?;
                    return Ok(quote! { #(#inner)* });
                }
                let e = self.emit_expr(expr)?;
                Ok(quote! { let _ = #e; })
            }
            IrStmtKind::Let {
                name,
                ty,
                type_annotation,
                mutability,
                value,
            } => {
                let binding_is_used = local_binding_is_used.unwrap_or(true);
                let emitted_name = if binding_is_used {
                    name.clone()
                } else {
                    format!("_{name}")
                };
                let n = Self::rust_ident(&emitted_name);
                let value_target_ty = type_annotation.as_ref().unwrap_or(ty);
                let v = self.emit_assignment_value(value, Some(value_target_ty))?;
                let converted_v = plan_value_use(
                    value,
                    ValueUseSite::Assignment {
                        target_ty: Some(value_target_ty),
                    },
                )
                .apply(v);
                let annotation = type_annotation
                    .as_ref()
                    .and_then(|annotated_ty| self.emit_local_let_annotation(annotated_ty));

                let binding_is_mutated_after = following_slices
                    .iter()
                    .any(|stmts| stmts.iter().any(|stmt| stmt_mutates_var(stmt, name)))
                    || following_expr
                        .as_ref()
                        .is_some_and(|expr| expr_contains_mutation(expr, name));
                let needs_mut = binding_is_used
                    && (matches!(mutability, Mutability::Mutable)
                        || binding_is_mutated_after
                        || matches!(value.kind, IrExprKind::StaticBinding { .. })
                            && self.current_storage_binding_needs_mut(name));
                if needs_mut {
                    Ok(quote! { let mut #n #annotation = #converted_v; })
                } else {
                    Ok(quote! { let #n #annotation = #converted_v; })
                }
            }
            IrStmtKind::Assign { target, value } => {
                if let AssignTarget::Static(name) = target {
                    let n = Self::rust_static_ident(name);
                    let init_call = self.emit_static_init_call_for_static(name);
                    let v = self.emit_assignment_value(value, None)?;
                    return Ok(quote! {
                        #init_call
                        let __incan_static_rhs = #v;
                        #n.with_mut(|__incan_static_value| {
                            *__incan_static_value = __incan_static_rhs.into();
                        });
                    });
                }

                if let AssignTarget::StaticBinding(name) = target {
                    return self.emit_static_binding_assignment(name, value);
                }

                let storage_rooted_target = match target {
                    AssignTarget::Field { object, .. } | AssignTarget::Index { object, .. } => {
                        Self::expr_is_storage_rooted(object)
                    }
                    _ => false,
                };
                if storage_rooted_target {
                    return self.emit_storage_rooted_assignment(target, value);
                }

                // For Dict index assignment, use .insert() instead of []=
                // because HashMap's IndexMut doesn't work with owned keys
                if let AssignTarget::Index { object, index } = target
                    && matches!(&object.ty, IrType::Dict(_, _) | IrType::Unknown)
                {
                    let o = self.emit_expr(object)?;
                    let (key_target_ty, value_target_ty) = match &object.ty {
                        IrType::Dict(key_ty, value_ty) => (Some(key_ty.as_ref()), Some(value_ty.as_ref())),
                        _ => (None, None),
                    };
                    let k = self.emit_expr_for_use(
                        index,
                        ValueUseSite::CollectionElement {
                            target_ty: key_target_ty,
                        },
                    )?;
                    let v = self.emit_assignment_value(value, value_target_ty)?;
                    let v = plan_value_use(
                        value,
                        ValueUseSite::CollectionElement {
                            target_ty: value_target_ty,
                        },
                    )
                    .apply(v);
                    return Ok(quote! { #o.insert(#k, #v); });
                }
                if let AssignTarget::Index { object, .. } = target
                    && let Some(value_target_ty) = list_index_assignment_element_type(&object.ty)
                {
                    let t = self.emit_assign_target(target)?;
                    let v = self.emit_expr_for_use(
                        value,
                        ValueUseSite::Assignment {
                            target_ty: Some(value_target_ty),
                        },
                    )?;
                    return Ok(quote! { #t = #v; });
                }
                let t = self.emit_assign_target(target)?;
                let v = self.emit_assignment_value(value, None)?;
                Ok(quote! { #t = #v; })
            }
            IrStmtKind::Return(Some(expr)) => {
                // Set return context so function calls inside can use move semantics
                *self.in_return_context.borrow_mut() = true;
                let converted = if let Some(return_type) = self.current_function_return_type.borrow().as_ref() {
                    if let Some(wrapped) = self.emit_union_wrapped_value(expr, return_type, true)? {
                        wrapped
                    } else {
                        self.emit_expr_for_use(
                            expr,
                            ValueUseSite::ReturnValue {
                                target_ty: Some(return_type),
                            },
                        )?
                    }
                } else {
                    self.emit_expr(expr)?
                };
                *self.in_return_context.borrow_mut() = false;

                if is_diverging_rust_error_call(expr) {
                    return Ok(quote! { #converted; });
                }
                Ok(quote! { return #converted; })
            }
            IrStmtKind::Return(None) => Ok(quote! { return; }),
            IrStmtKind::Yield(expr) => {
                let value = self.emit_expr(expr)?;
                Ok(quote! { __incan_yield.yield_value(#value); })
            }
            IrStmtKind::Break { label, value } => {
                let break_value = if let Some(value) = value {
                    Some(self.emit_expr(value)?)
                } else {
                    None
                };
                if let Some(l) = label {
                    let label_lifetime = syn::Lifetime::new(&format!("'{}", l), proc_macro2::Span::call_site());
                    if let Some(value) = break_value {
                        Ok(quote! { break #label_lifetime #value; })
                    } else {
                        Ok(quote! { break #label_lifetime; })
                    }
                } else if let Some(value) = break_value {
                    Ok(quote! { break #value; })
                } else {
                    Ok(quote! { break; })
                }
            }
            IrStmtKind::Continue(label) => {
                if let Some(l) = label {
                    let label_lifetime = syn::Lifetime::new(&format!("'{}", l), proc_macro2::Span::call_site());
                    Ok(quote! { continue #label_lifetime; })
                } else {
                    Ok(quote! { continue; })
                }
            }
            IrStmtKind::While {
                label: _,
                condition,
                body,
            } => {
                let body_stmts = self.emit_stmts(body)?;
                let is_infinite = matches!(condition.kind, IrExprKind::Bool(true));
                if is_infinite {
                    Ok(quote! {
                        loop {
                            #(#body_stmts)*
                        }
                    })
                } else {
                    let cond = self.emit_expr(condition)?;
                    Ok(quote! {
                        while #cond {
                            #(#body_stmts)*
                        }
                    })
                }
            }
            IrStmtKind::For {
                label: _,
                pattern,
                iterable,
                body,
            } => {
                let pat = self.emit_pattern(pattern);
                let body_stmts = self.emit_stmts(body)?;
                // For non-copy collections, iterate by reference to avoid move
                // This handles the common case where a collection is used multiple times
                // For primitive element types, use .iter().copied() to get values instead of references
                let needs_mut_items = for_body_needs_mut_iteration(pattern, body);
                let iterable_is_borrowable_lvalue = matches!(
                    &iterable.kind,
                    IrExprKind::Var { .. } | IrExprKind::Field { .. } | IrExprKind::Index { .. }
                );
                let item_is_user_enum = match &iterable.ty {
                    IrType::Ref(inner) | IrType::RefMut(inner) => match inner.as_ref() {
                        IrType::List(elem_ty) => self.type_is_user_enum(elem_ty),
                        _ => false,
                    },
                    IrType::List(elem_ty) => self.type_is_user_enum(elem_ty),
                    _ => false,
                };
                let iter_plan = plan_for_loop_iteration(
                    &iterable.ty,
                    iterable_is_borrowable_lvalue,
                    needs_mut_items,
                    item_is_user_enum,
                );
                if iter_plan == LoopIterationPlan::AsIs
                    && let Some(range_for) = self.emit_direct_range_for_stmt(&pat, iterable, &body_stmts)?
                {
                    return Ok(range_for);
                }
                let iter_expr = self.emit_for_iterable(iterable, iter_plan)?;
                Ok(quote! {
                    for #pat in #iter_expr {
                        #(#body_stmts)*
                    }
                })
            }
            IrStmtKind::Loop { label: _, body } => {
                let body_stmts = self.emit_stmts(body)?;
                Ok(quote! {
                    loop {
                        #(#body_stmts)*
                    }
                })
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond = self.emit_expr(condition)?;
                let then_stmts = self.emit_stmts(then_branch)?;
                if let Some(else_stmts) = else_branch {
                    let else_tokens = self.emit_stmts(else_stmts)?;
                    Ok(quote! {
                        if #cond {
                            #(#then_stmts)*
                        } else {
                            #(#else_tokens)*
                        }
                    })
                } else {
                    Ok(quote! {
                        if #cond {
                            #(#then_stmts)*
                        }
                    })
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                let scrut = self.emit_match_scrutinee(scrutinee)?;
                let arm_tokens: Vec<TokenStream> = arms
                    .iter()
                    .map(|arm| {
                        let pattern = erase_unused_pattern_bindings(&arm.pattern, arm);
                        let (pat, pattern_guard) = self.emit_pattern_for_scrutinee(&pattern, &scrutinee.ty);
                        let body = self.emit_match_arm_body(arm)?;
                        let guard = self.emit_match_arm_guard(arm, pattern_guard)?;
                        if let Some(guard) = guard {
                            Ok(quote! { #pat if #guard => #body })
                        } else {
                            Ok(quote! { #pat => #body })
                        }
                    })
                    .collect::<Result<_, _>>()?;
                Ok(quote! {
                    match #scrut {
                        #(#arm_tokens),*
                    }
                })
            }
            IrStmtKind::Block(stmts) => {
                let inner = self.emit_stmts(stmts)?;
                Ok(quote! {
                    {
                        #(#inner)*
                    }
                })
            }
            IrStmtKind::CompoundAssign { .. } => Err(EmitError::Unsupported(
                "CompoundAssign should be lowered into a regular assignment before emission".to_string(),
            )),
        }
    }

    /// Emit a direct range `for` statement without interpolating the entire range as one grouped expression.
    fn emit_direct_range_for_stmt(
        &self,
        pat: &TokenStream,
        iterable: &TypedExpr,
        body_stmts: &[TokenStream],
    ) -> Result<Option<TokenStream>, EmitError> {
        match &iterable.kind {
            IrExprKind::BuiltinCall {
                func: BuiltinFn::Range,
                args,
            } => self.emit_builtin_range_for_stmt(pat, args, body_stmts),
            IrExprKind::Call {
                func,
                args,
                canonical_path,
                callable_signature,
                ..
            } if canonical_path.is_none() && callable_signature.is_none() && Self::call_expr_is_builtin_range(func) => {
                let Some(positional) = Self::positional_call_args(args) else {
                    return Ok(None);
                };
                self.emit_builtin_range_for_stmt(pat, &positional, body_stmts)
            }
            IrExprKind::Range { start, end, inclusive } => {
                self.emit_range_expr_for_stmt(pat, start.as_deref(), end.as_deref(), *inclusive, body_stmts)
            }
            _ => Ok(None),
        }
    }

    /// Return whether a call callee names the builtin `range` without an already-resolved real function target.
    fn call_expr_is_builtin_range(func: &TypedExpr) -> bool {
        matches!(
            &func.kind,
            IrExprKind::Var { name, .. }
                if BuiltinFn::from_name(name) == Some(BuiltinFn::Range)
        )
    }

    /// Convert ordinary positional call arguments into expression values for builtin range emission.
    fn positional_call_args(args: &[super::super::expr::IrCallArg]) -> Option<Vec<TypedExpr>> {
        args.iter()
            .map(|arg| match arg.kind {
                IrCallArgKind::Positional => Some(arg.expr.clone()),
                IrCallArgKind::Named | IrCallArgKind::PositionalUnpack | IrCallArgKind::KeywordUnpack => None,
            })
            .collect()
    }

    /// Emit `for` over the `range(...)` builtin when it lowers to a direct Rust range.
    fn emit_builtin_range_for_stmt(
        &self,
        pat: &TokenStream,
        args: &[TypedExpr],
        body_stmts: &[TokenStream],
    ) -> Result<Option<TokenStream>, EmitError> {
        if args.len() == 1 {
            if let IrExprKind::Range { start, end, inclusive } = &args[0].kind {
                return self.emit_range_expr_for_stmt(pat, start.as_deref(), end.as_deref(), *inclusive, body_stmts);
            }
            let end = self.emit_expr(&args[0])?;
            return Ok(Some(quote! {
                for #pat in 0_i64..(#end as i64) {
                    #(#body_stmts)*
                }
            }));
        }

        if args.len() == 2 {
            let start = self.emit_expr(&args[0])?;
            let end = self.emit_expr(&args[1])?;
            return Ok(Some(quote! {
                for #pat in (#start as i64)..(#end as i64) {
                    #(#body_stmts)*
                }
            }));
        }

        if args.len() == 3 && matches!(&args[2].kind, IrExprKind::Int(1)) {
            let start = self.emit_expr(&args[0])?;
            let end = self.emit_expr(&args[1])?;
            return Ok(Some(quote! {
                for #pat in (#start as i64)..(#end as i64) {
                    #(#body_stmts)*
                }
            }));
        }

        Ok(None)
    }

    /// Emit `for` over an Incan range expression when it lowers to a direct Rust range.
    fn emit_range_expr_for_stmt(
        &self,
        pat: &TokenStream,
        start: Option<&TypedExpr>,
        end: Option<&TypedExpr>,
        inclusive: bool,
        body_stmts: &[TokenStream],
    ) -> Result<Option<TokenStream>, EmitError> {
        match (start, end, inclusive) {
            (Some(start), Some(end), false) => {
                let start = self.emit_expr(start)?;
                let end = self.emit_expr(end)?;
                Ok(Some(quote! {
                    for #pat in #start..#end {
                        #(#body_stmts)*
                    }
                }))
            }
            (Some(start), Some(end), true) => {
                let start = self.emit_expr(start)?;
                let end = self.emit_expr(end)?;
                Ok(Some(quote! {
                    for #pat in #start..=#end {
                        #(#body_stmts)*
                    }
                }))
            }
            (Some(start), None, _) => {
                let start = self.emit_expr(start)?;
                Ok(Some(quote! {
                    for #pat in #start.. {
                        #(#body_stmts)*
                    }
                }))
            }
            (None, Some(end), false) => {
                let end = self.emit_expr(end)?;
                Ok(Some(quote! {
                    for #pat in ..#end {
                        #(#body_stmts)*
                    }
                }))
            }
            (None, Some(end), true) => {
                let end = self.emit_expr(end)?;
                Ok(Some(quote! {
                    for #pat in ..=#end {
                        #(#body_stmts)*
                    }
                }))
            }
            (None, None, _) => Ok(Some(quote! {
                for #pat in .. {
                    #(#body_stmts)*
                }
            })),
        }
    }

    /// Emit a `for` iterable without introducing warning-prone grouping around direct Rust range iterators.
    ///
    /// The generic expression emitter may produce a range token stream that is grouped when substituted into a larger
    /// expression. That grouping is harmless in most positions but Rust warns on `for x in (a..b)`, so direct range
    /// iterables are emitted at the `for` site while all adapted iterables keep the normal ownership plan.
    fn emit_for_iterable(&self, iterable: &TypedExpr, iter_plan: LoopIterationPlan) -> Result<TokenStream, EmitError> {
        if iter_plan == LoopIterationPlan::AsIs {
            match &iterable.kind {
                IrExprKind::BuiltinCall {
                    func: BuiltinFn::Range,
                    args,
                } => {
                    if let Some(range) = self.emit_range_call(args)? {
                        return Ok(range);
                    }
                }
                IrExprKind::Range { start, end, inclusive } => {
                    return self.emit_range_expr(start.as_deref(), end.as_deref(), *inclusive);
                }
                _ => {}
            }
        }

        let iter = self.emit_expr(iterable)?;
        Ok(iter_plan.apply(iter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::FunctionRegistry;
    use crate::backend::ir::TypedExpr;
    use crate::backend::ir::expr::{CollectionMethodKind, IrCallArg, IrCallArgKind, MethodKind, VarAccess, VarRefKind};
    use crate::backend::ir::types::Mutability;

    #[test]
    fn immutable_static_binding_let_does_not_emit_mut() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let stmt = IrStmt::new(IrStmtKind::Let {
            name: "flags".to_string(),
            ty: IrType::List(Box::new(IrType::Bool)),
            type_annotation: None,
            mutability: Mutability::Immutable,
            value: TypedExpr::new(
                IrExprKind::StaticBinding {
                    name: "ACTIVE_FLAGS".to_string(),
                },
                IrType::List(Box::new(IrType::Bool)),
            ),
        });

        let emitted = emitter
            .emit_stmt(&stmt)
            .map_err(|err| format!("expected successful statement emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("let flags ="),
            "expected immutable static binding let emission, got `{rendered}`"
        );
        assert!(
            !rendered.contains("let mut flags"),
            "read-only static binding let must not emit `mut`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn source_mutable_let_still_emits_mut() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let stmt = IrStmt::new(IrStmtKind::Let {
            name: "flags".to_string(),
            ty: IrType::List(Box::new(IrType::Bool)),
            type_annotation: None,
            mutability: Mutability::Mutable,
            value: TypedExpr::new(
                IrExprKind::Var {
                    name: "flags_src".to_string(),
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                IrType::List(Box::new(IrType::Bool)),
            ),
        });

        let emitted = emitter
            .emit_stmt(&stmt)
            .map_err(|err| format!("expected successful statement emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("let mut flags ="),
            "source-mutable lets must emit Rust `mut`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn later_assignment_to_plain_local_emits_mut() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let stmts = vec![
            IrStmt::new(IrStmtKind::Let {
                name: "current".to_string(),
                ty: IrType::Int,
                type_annotation: None,
                mutability: Mutability::Immutable,
                value: TypedExpr::new(IrExprKind::Int(1), IrType::Int),
            }),
            IrStmt::new(IrStmtKind::Assign {
                target: AssignTarget::Var("current".to_string()),
                value: TypedExpr::new(IrExprKind::Int(2), IrType::Int),
            }),
        ];

        let emitted = emitter
            .emit_stmts(&stmts)
            .map_err(|err| format!("expected successful statement emission, got {err:?}"))?;
        let rendered = quote! { #(#emitted)* }.to_string();
        assert!(
            rendered.contains("let mut current ="),
            "later assignment to a local must emit Rust `mut`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn storage_mutated_static_binding_let_emits_mut_inside_statement_slice() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let stmts = vec![
            IrStmt::new(IrStmtKind::Let {
                name: "live".to_string(),
                ty: IrType::List(Box::new(IrType::Int)),
                type_annotation: None,
                mutability: Mutability::Immutable,
                value: TypedExpr::new(
                    IrExprKind::StaticBinding {
                        name: "ITEMS".to_string(),
                    },
                    IrType::List(Box::new(IrType::Int)),
                ),
            }),
            IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::KnownMethodCall {
                    receiver: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: "live".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::StaticBinding,
                        },
                        IrType::List(Box::new(IrType::Int)),
                    )),
                    kind: MethodKind::Collection(CollectionMethodKind::Append),
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Int(2), IrType::Int),
                    }],
                },
                IrType::Unit,
            ))),
        ];

        let emitted = emitter
            .emit_stmts(&stmts)
            .map_err(|err| format!("expected successful statement emission, got {err:?}"))?;
        let rendered = quote! { #(#emitted)* }.to_string();
        assert!(
            rendered.contains("let mut live ="),
            "storage-mutated static binding lets must emit `mut`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn storage_binding_analysis_matches_method_mutability_policy() -> Result<(), String> {
        let method_kinds = vec![
            CollectionMethodKind::Insert,
            CollectionMethodKind::Remove,
            CollectionMethodKind::Append,
            CollectionMethodKind::Extend,
            CollectionMethodKind::Pop,
            CollectionMethodKind::Swap,
            CollectionMethodKind::Reserve,
            CollectionMethodKind::ReserveExact,
            CollectionMethodKind::Get,
        ];

        for kind in method_kinds {
            let method_kind = MethodKind::Collection(kind);
            let mut names = HashSet::new();
            let stmt = IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
                IrExprKind::KnownMethodCall {
                    receiver: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: "live".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::StaticBinding,
                        },
                        IrType::List(Box::new(IrType::Int)),
                    )),
                    kind: method_kind,
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Int(1), IrType::Int),
                    }],
                },
                IrType::Unit,
            )));

            stmt_mutates_storage_binding(&stmt, &mut names);
            let expected = method_kind_uses_mutable_receiver(&method_kind);
            let observed = names.contains("live");
            if observed != expected {
                return Err(format!(
                    "storage-binding mutability analysis drifted for {method_kind:?}: expected {expected}, observed {observed}"
                ));
            }
        }

        Ok(())
    }
}
