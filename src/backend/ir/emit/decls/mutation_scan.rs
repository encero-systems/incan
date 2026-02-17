//! Parameter mutation analysis for function emission.
//!
//! Scans IR function bodies to determine which parameters are actually mutated, so Rust signatures emit
//! `mut`/`&mut`nonly where needed (avoiding Rust's "unused `mut`" warnings).

use std::collections::HashSet;

use std::sync::LazyLock;

use incan_core::lang::surface::methods::{dict_methods, list_methods};

use super::super::super::expr::{IrExpr, IrExprKind, MethodKind, VarAccess};
use super::super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::IrEmitter;

/// Method names that mutate their receiver, built from `incan_core` registries.
///
/// `push` and `clear` are Rust-internal method names emitted by lowering that don't have surface-level registry entries
/// — they're included explicitly.
static MUTATING_METHOD_NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        list_methods::as_str(list_methods::ListMethodId::Append),
        list_methods::as_str(list_methods::ListMethodId::Pop),
        list_methods::as_str(list_methods::ListMethodId::Swap),
        list_methods::as_str(list_methods::ListMethodId::Reserve),
        list_methods::as_str(list_methods::ListMethodId::ReserveExact),
        list_methods::as_str(list_methods::ListMethodId::Remove),
        dict_methods::as_str(dict_methods::DictMethodId::Insert),
        // TODO: Rust-internal method names with no surface registry entry
        "push",
        "clear",
    ]
});

impl<'a> IrEmitter<'a> {
    /// Collect the set of parameter names that are actually mutated in a function body.
    ///
    /// This is used to avoid emitting `mut`/`&mut` in Rust function signatures when the parameter is never written to,
    /// which would trigger Rust's "unused `mut`" warnings.
    pub(in crate::backend::ir::emit) fn collect_mutated_params(
        &self,
        func: &super::super::super::decl::IrFunction,
    ) -> HashSet<String> {
        let param_names: HashSet<String> = func.params.iter().map(|p| p.name.clone()).collect();
        let mut mutated: HashSet<String> = HashSet::new();

        for stmt in &func.body {
            self.scan_stmt_for_param_writes(stmt, &param_names, &mut mutated);
        }

        mutated
    }

    /// Scan an IR statement and record any writes that target a function parameter.
    fn scan_stmt_for_param_writes(&self, stmt: &IrStmt, param_names: &HashSet<String>, mutated: &mut HashSet<String>) {
        match &stmt.kind {
            IrStmtKind::Let { value, .. } => self.scan_expr_for_param_writes(value, param_names, mutated),
            IrStmtKind::Assign { target, value } => {
                if let Some(name) = self.assign_target_hits_param(target, param_names) {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(value, param_names, mutated);
            }
            IrStmtKind::CompoundAssign { target, value, .. } => {
                if let Some(name) = self.assign_target_hits_param(target, param_names) {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(value, param_names, mutated);
            }
            IrStmtKind::Expr(e) => self.scan_expr_for_param_writes(e, param_names, mutated),
            IrStmtKind::Return(Some(e)) => self.scan_expr_for_param_writes(e, param_names, mutated),
            IrStmtKind::Return(None) | IrStmtKind::Break(_) | IrStmtKind::Continue(_) => {}
            IrStmtKind::While { condition, body, .. } => {
                self.scan_expr_for_param_writes(condition, param_names, mutated);
                for s in body {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
            IrStmtKind::For { iterable, body, .. } => {
                self.scan_expr_for_param_writes(iterable, param_names, mutated);
                for s in body {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
            IrStmtKind::Loop { body, .. } => {
                for s in body {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr_for_param_writes(condition, param_names, mutated);
                for s in then_branch {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
                if let Some(else_branch) = else_branch {
                    for s in else_branch {
                        self.scan_stmt_for_param_writes(s, param_names, mutated);
                    }
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                self.scan_expr_for_param_writes(scrutinee, param_names, mutated);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.scan_expr_for_param_writes(guard, param_names, mutated);
                    }
                    self.scan_expr_for_param_writes(&arm.body, param_names, mutated);
                }
            }
            IrStmtKind::Block(stmts) => {
                for s in stmts {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
        }
    }

    /// Scan an IR expression and record any writes that target a function parameter.
    fn scan_expr_for_param_writes(&self, expr: &IrExpr, param_names: &HashSet<String>, mutated: &mut HashSet<String>) {
        match &expr.kind {
            IrExprKind::Var { name, access, .. } => {
                if *access == VarAccess::BorrowMut && param_names.contains(name) {
                    mutated.insert(name.clone());
                }
            }
            IrExprKind::BinOp { left, right, .. } => {
                self.scan_expr_for_param_writes(left, param_names, mutated);
                self.scan_expr_for_param_writes(right, param_names, mutated);
            }
            IrExprKind::UnaryOp { operand, .. } => {
                self.scan_expr_for_param_writes(operand, param_names, mutated);
            }
            IrExprKind::Call { func, args, .. } => {
                self.scan_expr_for_param_writes(func, param_names, mutated);
                for arg in args {
                    self.scan_expr_for_param_writes(&arg.expr, param_names, mutated);
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    self.scan_expr_for_param_writes(arg, param_names, mutated);
                }
            }
            IrExprKind::MethodCall { receiver, method, args } => {
                if let Some(name) = self.expr_is_param_var(receiver, param_names)
                    && Self::is_mutating_method_name(method)
                {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(receiver, param_names, mutated);
                for arg in args {
                    self.scan_expr_for_param_writes(&arg.expr, param_names, mutated);
                }
            }
            IrExprKind::KnownMethodCall {
                receiver, kind, args, ..
            } => {
                if let Some(name) = self.expr_is_param_var(receiver, param_names)
                    && Self::is_mutating_method_kind(kind)
                {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(receiver, param_names, mutated);
                for arg in args {
                    self.scan_expr_for_param_writes(&arg.expr, param_names, mutated);
                }
            }
            IrExprKind::Field { object, .. } => {
                self.scan_expr_for_param_writes(object, param_names, mutated);
            }
            IrExprKind::Index { object, index } => {
                self.scan_expr_for_param_writes(object, param_names, mutated);
                self.scan_expr_for_param_writes(index, param_names, mutated);
            }
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => {
                self.scan_expr_for_param_writes(target, param_names, mutated);
                if let Some(s) = start {
                    self.scan_expr_for_param_writes(s, param_names, mutated);
                }
                if let Some(e) = end {
                    self.scan_expr_for_param_writes(e, param_names, mutated);
                }
                if let Some(st) = step {
                    self.scan_expr_for_param_writes(st, param_names, mutated);
                }
            }
            IrExprKind::ListComp {
                element,
                iterable,
                filter,
                ..
            } => {
                self.scan_expr_for_param_writes(element, param_names, mutated);
                self.scan_expr_for_param_writes(iterable, param_names, mutated);
                if let Some(f) = filter {
                    self.scan_expr_for_param_writes(f, param_names, mutated);
                }
            }
            IrExprKind::DictComp {
                key,
                value,
                iterable,
                filter,
                ..
            } => {
                self.scan_expr_for_param_writes(key, param_names, mutated);
                self.scan_expr_for_param_writes(value, param_names, mutated);
                self.scan_expr_for_param_writes(iterable, param_names, mutated);
                if let Some(f) = filter {
                    self.scan_expr_for_param_writes(f, param_names, mutated);
                }
            }
            IrExprKind::List(items) | IrExprKind::Tuple(items) | IrExprKind::Set(items) => {
                for i in items {
                    self.scan_expr_for_param_writes(i, param_names, mutated);
                }
            }
            IrExprKind::Dict(pairs) => {
                for (k, v) in pairs {
                    self.scan_expr_for_param_writes(k, param_names, mutated);
                    self.scan_expr_for_param_writes(v, param_names, mutated);
                }
            }
            IrExprKind::Struct { fields, .. } => {
                for (_, v) in fields {
                    self.scan_expr_for_param_writes(v, param_names, mutated);
                }
            }
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr_for_param_writes(condition, param_names, mutated);
                self.scan_expr_for_param_writes(then_branch, param_names, mutated);
                if let Some(e) = else_branch {
                    self.scan_expr_for_param_writes(e, param_names, mutated);
                }
            }
            IrExprKind::Match { scrutinee, arms } => {
                self.scan_expr_for_param_writes(scrutinee, param_names, mutated);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.scan_expr_for_param_writes(guard, param_names, mutated);
                    }
                    self.scan_expr_for_param_writes(&arm.body, param_names, mutated);
                }
            }
            IrExprKind::Closure { body, .. } => {
                self.scan_expr_for_param_writes(body, param_names, mutated);
            }
            IrExprKind::Block { stmts, value } => {
                for s in stmts {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
                if let Some(v) = value {
                    self.scan_expr_for_param_writes(v, param_names, mutated);
                }
            }
            IrExprKind::Await(inner) | IrExprKind::Try(inner) | IrExprKind::Cast { expr: inner, .. } => {
                self.scan_expr_for_param_writes(inner, param_names, mutated);
            }
            IrExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.scan_expr_for_param_writes(s, param_names, mutated);
                }
                if let Some(e) = end {
                    self.scan_expr_for_param_writes(e, param_names, mutated);
                }
            }
            IrExprKind::Format { parts } => {
                for part in parts {
                    if let super::super::super::expr::FormatPart::Expr(e) = part {
                        self.scan_expr_for_param_writes(e, param_names, mutated);
                    }
                }
            }
            // Literals and variants without child expressions
            _ => {}
        }
    }

    /// Check if an assignment target hits a function parameter.
    fn assign_target_hits_param(&self, target: &AssignTarget, param_names: &HashSet<String>) -> Option<String> {
        match target {
            AssignTarget::Var(name) if param_names.contains(name) => Some(name.clone()),
            AssignTarget::Field { object, .. } | AssignTarget::Index { object, .. } => {
                self.expr_is_param_var(object, param_names)
            }
            _ => None,
        }
    }

    /// Check if an expression is a function parameter.
    fn expr_is_param_var(&self, expr: &IrExpr, param_names: &HashSet<String>) -> Option<String> {
        if let IrExprKind::Var { name, .. } = &expr.kind
            && param_names.contains(name)
        {
            return Some(name.clone());
        }
        None
    }

    /// Check if a method name is mutating.
    fn is_mutating_method_name(name: &str) -> bool {
        MUTATING_METHOD_NAMES.contains(&name)
    }

    /// Check if a method kind is mutating.
    fn is_mutating_method_kind(kind: &MethodKind) -> bool {
        matches!(
            kind,
            MethodKind::Append
                | MethodKind::Pop
                | MethodKind::Insert
                | MethodKind::Remove
                | MethodKind::Swap
                | MethodKind::Reserve
                | MethodKind::ReserveExact
        )
    }
}
