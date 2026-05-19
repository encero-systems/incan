//! Statement emission for IR to Rust code generation
//!
//! This module handles emitting Rust statements from IR statements,
//! including let bindings, assignments, control flow, and blocks.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;

use super::super::expr::{
    IrCallArg, IrDictEntry, IrExprKind, IrGeneratorClause, IrListEntry, MatchArm, Pattern, TypedExpr,
};
use super::super::ownership::{ValueUseSite, plan_for_loop_iteration, plan_value_use};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::super::types::Mutability;
use super::{EmitError, IrEmitter};
use crate::backend::ir::emit::expressions::method_kind_uses_mutable_receiver;

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
            IrExprKind::Block { stmts, value } => {
                stmts.iter().any(|s| stmt_mutates_var(s, var))
                    || value.as_ref().is_some_and(|v| expr_contains_mutation(v, var))
            }
            IrExprKind::Loop { body } => body.iter().any(|stmt| stmt_mutates_var(stmt, var)),
            IrExprKind::Race { arms, .. } => arms
                .iter()
                .any(|arm| expr_contains_mutation(&arm.awaitable, var) || expr_contains_mutation(&arm.body, var)),
            _ => false,
        }
    }

    /// Check if a statement mutates a variable.
    fn stmt_mutates_var(stmt: &IrStmt, var: &str) -> bool {
        match &stmt.kind {
            IrStmtKind::Assign { target, .. } => target_mutates_var(target, var),
            IrStmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                then_branch.iter().any(|s| stmt_mutates_var(s, var))
                    || else_branch
                        .as_ref()
                        .is_some_and(|b| b.iter().any(|s| stmt_mutates_var(s, var)))
            }
            IrStmtKind::While { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::For { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::Loop { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::Block(stmts) => stmts.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::Match { arms, .. } => arms.iter().any(|arm| expr_contains_mutation(&arm.body, var)),
            IrStmtKind::Break { label: _, value } => {
                value.as_ref().is_some_and(|value| expr_contains_mutation(value, var))
            }
            _ => false,
        }
    }

    body.iter().any(|s| stmt_mutates_var(s, loop_var))
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

#[derive(Clone, Copy)]
struct BindingUseScan {
    used: bool,
    shadowed_after: bool,
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

/// Scan local rest statements, outer leaked-binding tails, and an optional final block value for one binding.
fn binding_use_scan(
    local_rest: &[IrStmt],
    following_slices: &[&[IrStmt]],
    following_expr: Option<&TypedExpr>,
    binding_name: &str,
) -> BindingUseScan {
    let local_scan = stmt_list_binding_use_scan(local_rest, binding_name);
    if local_scan.used || local_scan.shadowed_after {
        return local_scan;
    }

    for slice in following_slices {
        let scan = stmt_list_binding_use_scan(slice, binding_name);
        if scan.used || scan.shadowed_after {
            return scan;
        }
    }

    BindingUseScan {
        used: following_expr.is_some_and(|expr| expr_uses_binding_name(expr, binding_name)),
        shadowed_after: false,
    }
}

/// Scan a sibling statement slice for references to one already-declared binding.
fn stmt_list_binding_use_scan(stmts: &[IrStmt], binding_name: &str) -> BindingUseScan {
    for stmt in stmts {
        let scan = stmt_binding_use_scan(stmt, binding_name);
        if scan.used || scan.shadowed_after {
            return scan;
        }
    }
    BindingUseScan {
        used: false,
        shadowed_after: false,
    }
}

/// Scan one statement for references to one local binding.
fn stmt_binding_use_scan(stmt: &IrStmt, binding_name: &str) -> BindingUseScan {
    match &stmt.kind {
        IrStmtKind::Expr(expr) => {
            if let IrExprKind::Block { stmts, value: None } = &expr.kind {
                return stmt_list_binding_use_scan(stmts, binding_name);
            }
            BindingUseScan {
                used: expr_uses_binding_name(expr, binding_name),
                shadowed_after: false,
            }
        }
        IrStmtKind::Yield(expr) => BindingUseScan {
            used: expr_uses_binding_name(expr, binding_name),
            shadowed_after: false,
        },
        IrStmtKind::Let { name, value, .. } => {
            if expr_uses_binding_name(value, binding_name) {
                return BindingUseScan {
                    used: true,
                    shadowed_after: false,
                };
            }
            BindingUseScan {
                used: false,
                shadowed_after: name == binding_name,
            }
        }
        IrStmtKind::Assign { target, value } => BindingUseScan {
            used: assign_target_uses_binding_name(target, binding_name) || expr_uses_binding_name(value, binding_name),
            shadowed_after: false,
        },
        IrStmtKind::CompoundAssign { target, value, .. } => BindingUseScan {
            used: assign_target_uses_binding_name(target, binding_name) || expr_uses_binding_name(value, binding_name),
            shadowed_after: false,
        },
        IrStmtKind::Return(Some(expr)) | IrStmtKind::Break { value: Some(expr), .. } => BindingUseScan {
            used: expr_uses_binding_name(expr, binding_name),
            shadowed_after: false,
        },
        IrStmtKind::While { condition, body, .. } => BindingUseScan {
            used: expr_uses_binding_name(condition, binding_name)
                || stmt_list_binding_use_scan(body, binding_name).used,
            shadowed_after: false,
        },
        IrStmtKind::For {
            pattern,
            iterable,
            body,
            ..
        } => {
            if pattern_uses_binding_name(pattern, binding_name) || expr_uses_binding_name(iterable, binding_name) {
                return BindingUseScan {
                    used: true,
                    shadowed_after: false,
                };
            }
            BindingUseScan {
                used: !pattern_binds_binding_name(pattern, binding_name)
                    && stmt_list_binding_use_scan(body, binding_name).used,
                shadowed_after: false,
            }
        }
        IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => BindingUseScan {
            used: stmt_list_binding_use_scan(body, binding_name).used,
            shadowed_after: false,
        },
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => BindingUseScan {
            used: expr_uses_binding_name(condition, binding_name)
                || stmt_list_binding_use_scan(then_branch, binding_name).used
                || else_branch
                    .as_ref()
                    .is_some_and(|branch| stmt_list_binding_use_scan(branch, binding_name).used),
            shadowed_after: false,
        },
        IrStmtKind::Match { scrutinee, arms } => {
            if expr_uses_binding_name(scrutinee, binding_name) {
                return BindingUseScan {
                    used: true,
                    shadowed_after: false,
                };
            }
            BindingUseScan {
                used: arms.iter().any(|arm| match_arm_uses_binding_name(arm, binding_name)),
                shadowed_after: false,
            }
        }
        IrStmtKind::Return(None) | IrStmtKind::Break { value: None, .. } | IrStmtKind::Continue(_) => BindingUseScan {
            used: false,
            shadowed_after: false,
        },
    }
}

/// Check whether an assignment target references one local binding.
fn assign_target_uses_binding_name(assign_target: &AssignTarget, binding_name: &str) -> bool {
    match assign_target {
        AssignTarget::Var(name) | AssignTarget::StaticBinding(name) => name == binding_name,
        AssignTarget::Static(_) => false,
        AssignTarget::Field { object, .. } => expr_uses_binding_name(object, binding_name),
        AssignTarget::Index { object, index } => {
            expr_uses_binding_name(object, binding_name) || expr_uses_binding_name(index, binding_name)
        }
    }
}

/// Check whether a call argument references one local binding.
fn call_arg_uses_binding_name(arg: &IrCallArg, binding_name: &str) -> bool {
    expr_uses_binding_name(&arg.expr, binding_name)
}

/// Check whether one match arm references one local binding.
fn match_arm_uses_binding_name(arm: &MatchArm, binding_name: &str) -> bool {
    if pattern_uses_binding_name(&arm.pattern, binding_name) {
        return true;
    }
    if pattern_binds_binding_name(&arm.pattern, binding_name) {
        return false;
    }
    arm.guard
        .as_ref()
        .is_some_and(|guard| expr_uses_binding_name(guard, binding_name))
        || expr_uses_binding_name(&arm.body, binding_name)
}

/// Check whether a pattern binds one local name.
fn pattern_binds_binding_name(pattern: &Pattern, binding_name: &str) -> bool {
    match pattern {
        Pattern::Var(name) => name == binding_name,
        Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
            items.iter().any(|item| pattern_binds_binding_name(item, binding_name))
        }
        Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|(_, pattern)| pattern_binds_binding_name(pattern, binding_name)),
        Pattern::Wildcard | Pattern::Literal(_) => false,
    }
}

/// Check whether non-binding pattern expressions reference one local binding.
fn pattern_uses_binding_name(pattern: &Pattern, binding_name: &str) -> bool {
    match pattern {
        Pattern::Literal(expr) => expr_uses_binding_name(expr, binding_name),
        Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
            items.iter().any(|item| pattern_uses_binding_name(item, binding_name))
        }
        Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|(_, pattern)| pattern_uses_binding_name(pattern, binding_name)),
        Pattern::Wildcard | Pattern::Var(_) => false,
    }
}

/// Check whether an expression references one local binding.
fn expr_uses_binding_name(expr: &super::super::expr::IrExpr, binding_name: &str) -> bool {
    match &expr.kind {
        IrExprKind::Var { name, .. } | IrExprKind::StaticRead { name } | IrExprKind::StaticBinding { name } => {
            name == binding_name
        }
        IrExprKind::BinOp { left, right, .. } => {
            expr_uses_binding_name(left, binding_name) || expr_uses_binding_name(right, binding_name)
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::NumericResize { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => expr_uses_binding_name(operand, binding_name),
        IrExprKind::Call { func, args, .. } => {
            expr_uses_binding_name(func, binding_name)
                || args.iter().any(|arg| call_arg_uses_binding_name(arg, binding_name))
        }
        IrExprKind::BuiltinCall { args, .. } | IrExprKind::Set(args) | IrExprKind::Tuple(args) => {
            args.iter().any(|arg| expr_uses_binding_name(arg, binding_name))
        }
        IrExprKind::MethodCall { receiver, args, .. } | IrExprKind::KnownMethodCall { receiver, args, .. } => {
            expr_uses_binding_name(receiver, binding_name)
                || args.iter().any(|arg| call_arg_uses_binding_name(arg, binding_name))
        }
        IrExprKind::Field { object, .. } => expr_uses_binding_name(object, binding_name),
        IrExprKind::Index { object, index } => {
            expr_uses_binding_name(object, binding_name) || expr_uses_binding_name(index, binding_name)
        }
        IrExprKind::Slice {
            target: sliced,
            start,
            end,
            step,
        } => {
            expr_uses_binding_name(sliced, binding_name)
                || start
                    .as_ref()
                    .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
                || end
                    .as_ref()
                    .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
                || step
                    .as_ref()
                    .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
        }
        IrExprKind::ListComp {
            element,
            pattern,
            iterable,
            filter,
            ..
        } => {
            expr_uses_binding_name(iterable, binding_name)
                || pattern_uses_binding_name(pattern, binding_name)
                || !pattern_binds_binding_name(pattern, binding_name)
                    && (expr_uses_binding_name(element, binding_name)
                        || filter
                            .as_ref()
                            .is_some_and(|expr| expr_uses_binding_name(expr, binding_name)))
        }
        IrExprKind::DictComp {
            key,
            value,
            pattern,
            iterable,
            filter,
            ..
        } => {
            expr_uses_binding_name(iterable, binding_name)
                || pattern_uses_binding_name(pattern, binding_name)
                || !pattern_binds_binding_name(pattern, binding_name)
                    && (expr_uses_binding_name(key, binding_name)
                        || expr_uses_binding_name(value, binding_name)
                        || filter
                            .as_ref()
                            .is_some_and(|expr| expr_uses_binding_name(expr, binding_name)))
        }
        IrExprKind::Generator { element, clauses } => {
            let mut used = false;
            for clause in clauses {
                match clause {
                    IrGeneratorClause::For { pattern, iterable } => {
                        used |= expr_uses_binding_name(iterable, binding_name)
                            || pattern_uses_binding_name(pattern, binding_name);
                        if pattern_binds_binding_name(pattern, binding_name) {
                            return used;
                        }
                    }
                    IrGeneratorClause::If(condition) => {
                        used |= expr_uses_binding_name(condition, binding_name);
                    }
                }
            }
            used || expr_uses_binding_name(element, binding_name)
        }
        IrExprKind::List(entries) => entries.iter().any(|entry| match entry {
            IrListEntry::Element(expr) | IrListEntry::Spread(expr) => expr_uses_binding_name(expr, binding_name),
        }),
        IrExprKind::Dict(entries) => entries.iter().any(|entry| match entry {
            IrDictEntry::Pair(key, value) => {
                expr_uses_binding_name(key, binding_name) || expr_uses_binding_name(value, binding_name)
            }
            IrDictEntry::Spread(expr) => expr_uses_binding_name(expr, binding_name),
        }),
        IrExprKind::Struct { fields, .. } => fields
            .iter()
            .any(|(_, expr)| expr_uses_binding_name(expr, binding_name)),
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_uses_binding_name(condition, binding_name)
                || expr_uses_binding_name(then_branch, binding_name)
                || else_branch
                    .as_ref()
                    .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
        }
        IrExprKind::Match { scrutinee, arms } => {
            expr_uses_binding_name(scrutinee, binding_name)
                || arms.iter().any(|arm| match_arm_uses_binding_name(arm, binding_name))
        }
        IrExprKind::Race { arms, binding } => {
            arms.iter()
                .any(|arm| expr_uses_binding_name(&arm.awaitable, binding_name))
                || binding != binding_name && arms.iter().any(|arm| expr_uses_binding_name(&arm.body, binding_name))
        }
        IrExprKind::Closure { params, body, captures } => {
            captures.iter().any(|capture| capture == binding_name)
                || !params.iter().any(|(name, _)| name == binding_name) && expr_uses_binding_name(body, binding_name)
        }
        IrExprKind::Block { stmts, value } => {
            let stmt_scan = stmt_list_binding_use_scan(stmts, binding_name);
            stmt_scan.used
                || !stmt_scan.shadowed_after
                    && value
                        .as_ref()
                        .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
        }
        IrExprKind::Loop { body } => stmt_list_binding_use_scan(body, binding_name).used,
        IrExprKind::Range { start, end, .. } => {
            start
                .as_ref()
                .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
                || end
                    .as_ref()
                    .is_some_and(|expr| expr_uses_binding_name(expr, binding_name))
        }
        IrExprKind::Format { parts } => parts.iter().any(|part| match part {
            super::super::expr::FormatPart::Expr(expr) => expr_uses_binding_name(expr, binding_name),
            super::super::expr::FormatPart::Literal(_) => false,
        }),
        IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Decimal(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::AssociatedFunction { .. }
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => false,
    }
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
        let emitted = stmts
            .iter()
            .enumerate()
            .map(|(index, stmt)| {
                let mut next_slices = Vec::with_capacity(following_slices.len() + 1);
                next_slices.push(&stmts[index + 1..]);
                next_slices.extend_from_slice(following_slices);
                self.emit_stmt_with_local_usage(stmt, local_usage[index], &next_slices, following_expr)
            })
            .collect::<Result<Vec<_>, _>>();
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
                object: Box::new(Self::rewrite_storage_root_expr(object, local_name)),
                field: field.clone(),
            },
            AssignTarget::Index { object, index } => AssignTarget::Index {
                object: Box::new(Self::rewrite_storage_root_expr(object, local_name)),
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
                Ok(quote! { #e; })
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

                let needs_mut = binding_is_used
                    && (matches!(mutability, Mutability::Mutable)
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
                    let v = self.emit_assignment_value(value, None)?;
                    return Ok(quote! {
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
                let iter = self.emit_expr(iterable)?;
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
                let iter_expr = iter_plan.apply(iter);
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
                        let (pat, pattern_guard) = self.emit_pattern_for_scrutinee(&arm.pattern, &scrutinee.ty);
                        let body = self.emit_expr(&arm.body)?;
                        let guard = match (&pattern_guard, &arm.guard) {
                            (Some(pattern_guard), Some(arm_guard)) => {
                                let arm_guard = self.emit_expr(arm_guard)?;
                                Some(quote! { (#pattern_guard) && (#arm_guard) })
                            }
                            (Some(pattern_guard), None) => Some(pattern_guard.clone()),
                            (None, Some(arm_guard)) => Some(self.emit_expr(arm_guard)?),
                            (None, None) => None,
                        };
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::FunctionRegistry;
    use crate::backend::ir::TypedExpr;
    use crate::backend::ir::expr::{CollectionMethodKind, IrCallArg, IrCallArgKind, MethodKind, VarAccess, VarRefKind};

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
    fn mutable_static_binding_let_still_emits_mut() -> Result<(), String> {
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
            "mutable lets must still emit `mut`, got `{rendered}`"
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
