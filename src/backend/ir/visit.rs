//! Mutable IR traversal shared by ownership analysis and its signature updates.

use super::expr::{IrDictEntry, IrListEntry, Pattern};
use super::stmt::AssignTarget;
use super::{IrExpr, IrExprKind, IrStmt, IrStmtKind};

/// Override a node hook and delegate to the walker to visit its children.
pub(crate) trait Visitor {
    /// Visit an expression, including nested statements and closure bodies.
    fn expr(&mut self, expr: &mut IrExpr) {
        walk_expr(expr, self);
    }
    /// Visit pattern bindings and literal expressions.
    fn pattern(&mut self, pattern: &mut Pattern) {
        walk_pattern(pattern, self);
    }
    /// Visit a statement and its expression children.
    fn stmt(&mut self, stmt: &mut IrStmt) {
        walk_stmt(stmt, self);
    }
}

/// Traverse a statement's children without revisiting the statement itself.
pub(crate) fn walk_stmt<V: Visitor + ?Sized>(stmt: &mut IrStmt, visitor: &mut V) {
    match &mut stmt.kind {
        IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) | IrStmtKind::Yield(expr) => {
            visitor.expr(expr);
        }
        IrStmtKind::Let { value, .. } => visitor.expr(value),
        IrStmtKind::Assign { target, value } | IrStmtKind::CompoundAssign { target, value, .. } => {
            walk_target(target, visitor);
            visitor.expr(value);
        }
        IrStmtKind::Break { value, .. } => {
            if let Some(expr) = value {
                visitor.expr(expr);
            }
        }
        IrStmtKind::While { condition, body, .. } => {
            visitor.expr(condition);
            for stmt in body {
                visitor.stmt(stmt);
            }
        }
        IrStmtKind::For {
            pattern,
            iterable,
            body,
            ..
        } => {
            visitor.pattern(pattern);
            visitor.expr(iterable);
            for stmt in body {
                visitor.stmt(stmt);
            }
        }
        IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => {
            for stmt in body {
                visitor.stmt(stmt);
            }
        }
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            visitor.expr(condition);
            for stmt in then_branch {
                visitor.stmt(stmt);
            }
            if let Some(branch) = else_branch {
                for stmt in branch {
                    visitor.stmt(stmt);
                }
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            visitor.expr(scrutinee);
            for arm in arms {
                visitor.pattern(&mut arm.pattern);
                for binding in &mut arm.bindings {
                    visitor.expr(&mut binding.value);
                    if let Some(guard_value) = &mut binding.guard_value {
                        visitor.expr(guard_value);
                    }
                }
                if let Some(guard) = &mut arm.guard {
                    visitor.expr(guard);
                }
                visitor.expr(&mut arm.body);
            }
        }
        IrStmtKind::Return(None) | IrStmtKind::Continue(_) => {}
    }
}

/// Traverse assignment-place expressions. Local destination types are owned by the statement hook.
fn walk_target<V: Visitor + ?Sized>(target: &mut AssignTarget, visitor: &mut V) {
    match target {
        AssignTarget::Field { object, .. } => visitor.expr(object),
        AssignTarget::Index { object, index } => {
            visitor.expr(object);
            visitor.expr(index);
        }
        _ => {}
    }
}

/// Traverse expression children without revisiting the expression itself.
pub(crate) fn walk_expr<V: Visitor + ?Sized>(expr: &mut IrExpr, visitor: &mut V) {
    match &mut expr.kind {
        IrExprKind::BinOp { left, right, .. } => {
            visitor.expr(left);
            visitor.expr(right);
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::NumericResize { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => {
            visitor.expr(operand);
        }
        IrExprKind::Call { func, args, .. } => {
            visitor.expr(func);
            for arg in args {
                visitor.expr(&mut arg.expr);
            }
        }
        IrExprKind::RegisterCallableName { callable, .. } => {
            visitor.expr(callable);
        }
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
            visitor.expr(value);
        }
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                visitor.expr(arg);
            }
        }
        IrExprKind::MethodCall { receiver, args, .. } | IrExprKind::KnownMethodCall { receiver, args, .. } => {
            visitor.expr(receiver);
            for arg in args {
                visitor.expr(&mut arg.expr);
            }
        }
        IrExprKind::Field { object, .. } => visitor.expr(object),
        IrExprKind::Index { object, index } => {
            visitor.expr(object);
            visitor.expr(index);
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            visitor.expr(target);
            if let Some(expr) = start {
                visitor.expr(expr);
            }
            if let Some(expr) = end {
                visitor.expr(expr);
            }
            if let Some(expr) = step {
                visitor.expr(expr);
            }
        }
        IrExprKind::ListComp {
            element,
            pattern,
            iterable,
            filter,
            ..
        } => {
            visitor.pattern(pattern);
            visitor.expr(element);
            visitor.expr(iterable);
            if let Some(expr) = filter {
                visitor.expr(expr);
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            pattern,
            iterable,
            filter,
            ..
        } => {
            visitor.pattern(pattern);
            visitor.expr(key);
            visitor.expr(value);
            visitor.expr(iterable);
            if let Some(expr) = filter {
                visitor.expr(expr);
            }
        }
        IrExprKind::Generator { element, clauses } => {
            visitor.expr(element);
            for clause in clauses {
                match clause {
                    super::expr::IrGeneratorClause::For { pattern, iterable } => {
                        visitor.pattern(pattern);
                        visitor.expr(iterable);
                    }
                    super::expr::IrGeneratorClause::If(condition) => {
                        visitor.expr(condition);
                    }
                }
            }
        }
        IrExprKind::List(items) => {
            for item in items {
                match item {
                    IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                        visitor.expr(value);
                    }
                }
            }
        }
        IrExprKind::Dict(entries) => {
            for entry in entries {
                match entry {
                    IrDictEntry::Pair(key, value) => {
                        visitor.expr(key);
                        visitor.expr(value);
                    }
                    IrDictEntry::Spread(value) => visitor.expr(value),
                }
            }
        }
        IrExprKind::Set(items) | IrExprKind::Tuple(items) => {
            for item in items {
                visitor.expr(item);
            }
        }
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                visitor.expr(value);
            }
        }
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            visitor.expr(condition);
            visitor.expr(then_branch);
            if let Some(expr) = else_branch {
                visitor.expr(expr);
            }
        }
        IrExprKind::Match { scrutinee, arms } => {
            visitor.expr(scrutinee);
            for arm in arms {
                visitor.pattern(&mut arm.pattern);
                for binding in &mut arm.bindings {
                    visitor.expr(&mut binding.value);
                    if let Some(guard_value) = &mut binding.guard_value {
                        visitor.expr(guard_value);
                    }
                }
                if let Some(guard) = &mut arm.guard {
                    visitor.expr(guard);
                }
                visitor.expr(&mut arm.body);
            }
        }
        IrExprKind::Race { arms, .. } => {
            for arm in arms {
                visitor.expr(&mut arm.awaitable);
                visitor.expr(&mut arm.body);
            }
        }
        IrExprKind::Closure { body, .. } => visitor.expr(body),
        IrExprKind::Block { stmts, value } => {
            for stmt in stmts {
                visitor.stmt(stmt);
            }
            if let Some(expr) = value {
                visitor.expr(expr);
            }
        }
        IrExprKind::Loop { body } => {
            for stmt in body {
                visitor.stmt(stmt);
            }
        }
        IrExprKind::Range { start, end, .. } => {
            if let Some(expr) = start {
                visitor.expr(expr);
            }
            if let Some(expr) = end {
                visitor.expr(expr);
            }
        }
        IrExprKind::Format { parts } => {
            for part in parts {
                if let super::expr::FormatPart::Expr { expr, .. } = part {
                    visitor.expr(expr);
                }
            }
        }
        IrExprKind::EmbeddedFragment { holes, .. } => {
            for hole in holes {
                visitor.expr(hole);
            }
        }
        IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Decimal(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::Var { .. }
        | IrExprKind::StaticRead { .. }
        | IrExprKind::StaticBinding { .. }
        | IrExprKind::AssociatedFunction { .. }
        | IrExprKind::TypeToken { .. }
        | IrExprKind::FunctionItem { .. }
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => {}
    }
}

/// Traverse every pattern child, including executable literal expressions retained by lowering.
pub(crate) fn walk_pattern<V: Visitor + ?Sized>(pattern: &mut Pattern, visitor: &mut V) {
    match pattern {
        Pattern::Literal(value) => visitor.expr(value),
        Pattern::Tuple(fields) | Pattern::Or(fields) | Pattern::Enum { fields, .. } => {
            for field in fields {
                visitor.pattern(field);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, field) in fields {
                visitor.pattern(field);
            }
        }
        Pattern::Wildcard | Pattern::Var(_) => {}
    }
}
