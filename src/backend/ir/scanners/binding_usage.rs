//! Binding-use scanners for IR expressions and statements.

use crate::backend::ir::expr::{
    FormatPart, IrCallArg, IrDictEntry, IrExpr, IrExprKind, IrGeneratorClause, IrListEntry, MatchArm, Pattern,
};
use crate::backend::ir::stmt::{AssignTarget, IrStmt, IrStmtKind};

#[derive(Clone, Copy)]
pub(crate) struct BindingUseScan {
    pub(crate) used: bool,
    pub(crate) shadowed_after: bool,
}

/// Scan local rest statements, outer leaked-binding tails, and an optional final block value for one binding.
pub(crate) fn binding_use_scan(
    local_rest: &[IrStmt],
    following_slices: &[&[IrStmt]],
    following_expr: Option<&IrExpr>,
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
pub(crate) fn stmt_list_binding_use_scan(stmts: &[IrStmt], binding_name: &str) -> BindingUseScan {
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

/// Check whether an expression references one local binding.
pub(crate) fn expr_uses_binding_name(expr: &IrExpr, binding_name: &str) -> bool {
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
            target,
            start,
            end,
            step,
        } => {
            expr_uses_binding_name(target, binding_name)
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
            FormatPart::Expr { expr, .. } => expr_uses_binding_name(expr, binding_name),
            FormatPart::Literal(_) => false,
        }),
        IrExprKind::RegisterCallableName { callable, .. } => expr_uses_binding_name(callable, binding_name),
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => expr_uses_binding_name(value, binding_name),
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
        | IrExprKind::TypeToken { .. }
        | IrExprKind::FunctionItem { .. }
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => false,
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
    arm.bindings.iter().any(|binding| {
        expr_uses_binding_name(&binding.value, binding_name)
            || binding
                .guard_value
                .as_ref()
                .is_some_and(|guard_value| expr_uses_binding_name(guard_value, binding_name))
    }) || arm
        .guard
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
