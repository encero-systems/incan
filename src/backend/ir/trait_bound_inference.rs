//! RFC 023: Trait bound inference for generic functions.
//!
//! This module scans IR function bodies to infer which Rust trait bounds are required on each type parameter based on
//! how the parameter is used (e.g., `==` requires `PartialEq`, f-string interpolation requires `Display`).
//!
//! ## Inference rules (from RFC 023)
//!
//! | Incan operation             | Inferred Rust trait bound      |
//! | --------------------------- | ------------------------------ |
//! | `==`, `!=`                  | `PartialEq`                    |
//! | `<`, `<=`, `>`, `>=`        | `PartialOrd`                   |
//! | f-string interpolation      | `std::fmt::Display`            |
//! | `+`                         | `std::ops::Add<Output = T>`    |
//! | `-`                         | `std::ops::Sub<Output = T>`    |
//! | `*`                         | `std::ops::Mul<Output = T>`    |
//! | `/`                         | `std::ops::Div<Output = T>`    |
//! | `%`                         | `std::ops::Rem<Output = T>`    |
//! | `clone()`                   | `Clone`                        |
//! | used as `Dict` key          | `Eq + Hash`                    |
//! | used as `Set` element       | `Eq + Hash`                    |
//!
//! ## Transitive inference
//!
//! If `foo[T]` calls `bar[T]` and `bar` requires `PartialEq`, then `foo` also requires `PartialEq` on `T`. This is
//! handled by collecting bounds from called generic functions.

use std::collections::{HashMap, HashSet};

use incan_core::lang::trait_bounds::rust as tb;

use super::IrProgram;
use super::decl::{FunctionParam, IrDeclKind, IrFunction, IrTraitBound, IrTypeParam};
use super::expr::{BinOp, FormatPart, IrExpr, IrExprKind};
use super::stmt::{IrStmt, IrStmtKind};
use super::types::IrType;

/// Run trait bound inference on an entire IR program.
///
/// This mutates the `type_params` of each generic function to include inferred bounds in addition to any explicit
/// `with` bounds from the source.
pub fn infer_trait_bounds(program: &mut IrProgram) {
    // ---- Pass 1: collect explicit + body-scanned bounds per function ----
    let mut function_bounds: HashMap<String, Vec<IrTypeParam>> = HashMap::new();
    let mut function_params: HashMap<String, Vec<FunctionParam>> = HashMap::new();

    for decl in &program.declarations {
        if let IrDeclKind::Function(func) = &decl.kind
            && !func.type_params.is_empty()
        {
            let inferred = infer_function_bounds(func);
            function_bounds.insert(func.name.clone(), inferred);
            function_params.insert(func.name.clone(), func.params.clone());
        }
    }

    // ---- Pass 2: transitive inference (propagate bounds from called generic functions) ----
    // We iterate until a fixed point is reached (no new bounds added). Clone-per-iteration avoids borrow conflicts
    // between reading callee bounds and writing caller bounds.
    let max_iterations = 20; // safety cap
    for _ in 0..max_iterations {
        let mut changed = false;
        let snapshot = function_bounds.clone();

        for decl in &program.declarations {
            if let IrDeclKind::Function(func) = &decl.kind {
                if func.type_params.is_empty() {
                    continue;
                }
                let called_generics = collect_called_generic_functions(func, &snapshot, &function_params);
                if let Some(current_bounds) = function_bounds.get_mut(&func.name) {
                    for (callee_name, type_arg_mapping) in &called_generics {
                        if let Some(callee_bounds) = snapshot.get(callee_name)
                            && propagate_transitive_bounds(current_bounds, callee_bounds, type_arg_mapping)
                        {
                            changed = true;
                        }
                    }
                }
            }
        }

        if !changed {
            break;
        }
    }

    // ---- Pass 3: write inferred bounds back into the IR ----
    for decl in &mut program.declarations {
        if let IrDeclKind::Function(func) = &mut decl.kind
            && let Some(inferred) = function_bounds.remove(&func.name)
        {
            func.type_params = inferred;
        }
    }
}

/// Infer trait bounds for a single function by scanning its body.
fn infer_function_bounds(func: &IrFunction) -> Vec<IrTypeParam> {
    let type_param_names: HashSet<&str> = func.type_params.iter().map(|tp| tp.name.as_str()).collect();
    let mut bounds_map: HashMap<String, Vec<IrTraitBound>> = HashMap::new();

    // Start with explicit bounds from `with` clauses.
    for tp in &func.type_params {
        bounds_map.insert(tp.name.clone(), tp.bounds.clone());
    }

    // Scan body statements for operations on type parameters.
    for stmt in &func.body {
        scan_stmt_for_bounds(stmt, &type_param_names, &func.params, &mut bounds_map);
    }

    // Rebuild type params with combined bounds.
    func.type_params
        .iter()
        .map(|tp| {
            let bounds = bounds_map.remove(&tp.name).unwrap_or_default();
            IrTypeParam {
                name: tp.name.clone(),
                bounds: deduplicate_bounds(bounds),
            }
        })
        .collect()
}

/// Scan a statement for trait-bound-relevant operations on type parameters.
fn scan_stmt_for_bounds(
    stmt: &IrStmt,
    type_params: &HashSet<&str>,
    params: &[super::decl::FunctionParam],
    bounds_map: &mut HashMap<String, Vec<IrTraitBound>>,
) {
    match &stmt.kind {
        IrStmtKind::Expr(expr) => scan_expr_for_bounds(expr, type_params, params, bounds_map),
        IrStmtKind::Let { value, .. } => scan_expr_for_bounds(value, type_params, params, bounds_map),
        IrStmtKind::Assign { value, .. } => scan_expr_for_bounds(value, type_params, params, bounds_map),
        IrStmtKind::CompoundAssign { value, .. } => {
            scan_expr_for_bounds(value, type_params, params, bounds_map);
        }
        IrStmtKind::Return(Some(expr)) => scan_expr_for_bounds(expr, type_params, params, bounds_map),
        IrStmtKind::Return(None) | IrStmtKind::Break(_) | IrStmtKind::Continue(_) => {}
        IrStmtKind::While { condition, body, .. } => {
            scan_expr_for_bounds(condition, type_params, params, bounds_map);
            for s in body {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
        IrStmtKind::For { iterable, body, .. } => {
            scan_expr_for_bounds(iterable, type_params, params, bounds_map);
            for s in body {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
        IrStmtKind::Loop { body, .. } => {
            for s in body {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            scan_expr_for_bounds(condition, type_params, params, bounds_map);
            for s in then_branch {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(else_stmts) = else_branch {
                for s in else_stmts {
                    scan_stmt_for_bounds(s, type_params, params, bounds_map);
                }
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            scan_expr_for_bounds(scrutinee, type_params, params, bounds_map);
            for arm in arms {
                scan_expr_for_bounds(&arm.body, type_params, params, bounds_map);
                if let Some(guard) = &arm.guard {
                    scan_expr_for_bounds(guard, type_params, params, bounds_map);
                }
            }
        }
        IrStmtKind::Block(stmts) => {
            for s in stmts {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
        }
    }
}

/// Scan an expression for trait-bound-relevant operations on type parameters.
fn scan_expr_for_bounds(
    expr: &IrExpr,
    type_params: &HashSet<&str>,
    params: &[super::decl::FunctionParam],
    bounds_map: &mut HashMap<String, Vec<IrTraitBound>>,
) {
    match &expr.kind {
        // ---- Binary operations: check if either operand is a type parameter ----
        IrExprKind::BinOp { op, left, right } => {
            let left_tp = expr_type_param_name(left, type_params, params);
            let right_tp = expr_type_param_name(right, type_params, params);

            for tp_name in left_tp.iter().chain(right_tp.iter()) {
                if let Some(bound) = binop_to_trait_bound(op, tp_name) {
                    add_bound(bounds_map, tp_name, bound);
                }
            }

            scan_expr_for_bounds(left, type_params, params, bounds_map);
            scan_expr_for_bounds(right, type_params, params, bounds_map);
        }

        // ---- f-string interpolation: expressions used in format require Display ----
        IrExprKind::Format { parts } => {
            for part in parts {
                if let FormatPart::Expr(inner) = part {
                    if let Some(tp_name) = expr_type_param_name(inner, type_params, params) {
                        add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::DISPLAY));
                    }
                    scan_expr_for_bounds(inner, type_params, params, bounds_map);
                }
            }
        }

        // ---- Method call: `x.clone()` on a generic param requires Clone ----
        IrExprKind::MethodCall {
            receiver, method, args, ..
        } => {
            if method == "clone"
                && let Some(tp_name) = expr_type_param_name(receiver, type_params, params)
            {
                add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::CLONE));
            }
            scan_expr_for_bounds(receiver, type_params, params, bounds_map);
            for arg in args {
                scan_expr_for_bounds(&arg.expr, type_params, params, bounds_map);
            }
        }

        // ---- Function call: recurse into args ----
        IrExprKind::Call { func, args, .. } => {
            scan_expr_for_bounds(func, type_params, params, bounds_map);
            for arg in args {
                scan_expr_for_bounds(&arg.expr, type_params, params, bounds_map);
            }
        }

        // ---- Known method calls: recurse ----
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            scan_expr_for_bounds(receiver, type_params, params, bounds_map);
            for arg in args {
                scan_expr_for_bounds(&arg.expr, type_params, params, bounds_map);
            }
        }

        // ---- Builtin calls: recurse ----
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                scan_expr_for_bounds(arg, type_params, params, bounds_map);
            }
        }

        // ---- Dict literal: keys that are generic require Eq + Hash ----
        // Note: `Eq: PartialEq` in Rust, so we only need `Eq` (not redundant `PartialEq`).
        IrExprKind::Dict(entries) => {
            for (key, value) in entries {
                if let Some(tp_name) = expr_type_param_name(key, type_params, params) {
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::EQ));
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::HASH));
                }
                scan_expr_for_bounds(key, type_params, params, bounds_map);
                scan_expr_for_bounds(value, type_params, params, bounds_map);
            }
        }

        // ---- Set literal: elements that are generic require Eq + Hash ----
        IrExprKind::Set(elems) => {
            for elem in elems {
                if let Some(tp_name) = expr_type_param_name(elem, type_params, params) {
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::EQ));
                    add_bound(bounds_map, &tp_name, IrTraitBound::simple(tb::HASH));
                }
                scan_expr_for_bounds(elem, type_params, params, bounds_map);
            }
        }

        // ---- If expression: recurse ----
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            scan_expr_for_bounds(condition, type_params, params, bounds_map);
            scan_expr_for_bounds(then_branch, type_params, params, bounds_map);
            if let Some(e) = else_branch {
                scan_expr_for_bounds(e, type_params, params, bounds_map);
            }
        }

        // ---- Unary: recurse ----
        IrExprKind::UnaryOp { operand, .. } => {
            scan_expr_for_bounds(operand, type_params, params, bounds_map);
        }

        // ---- Field/Index: recurse ----
        IrExprKind::Field { object, .. } => scan_expr_for_bounds(object, type_params, params, bounds_map),
        IrExprKind::Index { object, index } => {
            scan_expr_for_bounds(object, type_params, params, bounds_map);
            scan_expr_for_bounds(index, type_params, params, bounds_map);
        }

        // ---- Collections: recurse ----
        IrExprKind::List(elems) | IrExprKind::Tuple(elems) => {
            for elem in elems {
                scan_expr_for_bounds(elem, type_params, params, bounds_map);
            }
        }

        // ---- Block: recurse into stmts and value ----
        IrExprKind::Block { stmts, value } => {
            for s in stmts {
                scan_stmt_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(v) = value {
                scan_expr_for_bounds(v, type_params, params, bounds_map);
            }
        }

        // ---- Match: recurse ----
        IrExprKind::Match { scrutinee, arms } => {
            scan_expr_for_bounds(scrutinee, type_params, params, bounds_map);
            for arm in arms {
                scan_expr_for_bounds(&arm.body, type_params, params, bounds_map);
                if let Some(guard) = &arm.guard {
                    scan_expr_for_bounds(guard, type_params, params, bounds_map);
                }
            }
        }

        // ---- Closure: recurse into body ----
        IrExprKind::Closure { body, .. } => {
            scan_expr_for_bounds(body, type_params, params, bounds_map);
        }

        // ---- ListComp / DictComp ----
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            scan_expr_for_bounds(element, type_params, params, bounds_map);
            scan_expr_for_bounds(iterable, type_params, params, bounds_map);
            if let Some(f) = filter {
                scan_expr_for_bounds(f, type_params, params, bounds_map);
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            scan_expr_for_bounds(key, type_params, params, bounds_map);
            scan_expr_for_bounds(value, type_params, params, bounds_map);
            scan_expr_for_bounds(iterable, type_params, params, bounds_map);
            if let Some(f) = filter {
                scan_expr_for_bounds(f, type_params, params, bounds_map);
            }
        }

        // ---- Struct construction: recurse into field values ----
        IrExprKind::Struct { fields, .. } => {
            for (_, val) in fields {
                scan_expr_for_bounds(val, type_params, params, bounds_map);
            }
        }

        // ---- Await/Try: recurse ----
        IrExprKind::Await(inner) | IrExprKind::Try(inner) => {
            scan_expr_for_bounds(inner, type_params, params, bounds_map);
        }

        // ---- Slice: recurse ----
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            scan_expr_for_bounds(target, type_params, params, bounds_map);
            if let Some(s) = start {
                scan_expr_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(e) = end {
                scan_expr_for_bounds(e, type_params, params, bounds_map);
            }
            if let Some(s) = step {
                scan_expr_for_bounds(s, type_params, params, bounds_map);
            }
        }

        // ---- Cast: recurse ----
        IrExprKind::Cast { expr, .. } => {
            scan_expr_for_bounds(expr, type_params, params, bounds_map);
        }

        // ---- Range: recurse ----
        IrExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                scan_expr_for_bounds(s, type_params, params, bounds_map);
            }
            if let Some(e) = end {
                scan_expr_for_bounds(e, type_params, params, bounds_map);
            }
        }

        // ---- Leaf nodes: no sub-expressions to scan ----
        IrExprKind::Var { .. }
        | IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::Float(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => {}
    }
}

/// Determine if an expression refers to a variable whose type is a type parameter.
///
/// Returns the type parameter name if so.
fn expr_type_param_name(
    expr: &IrExpr,
    type_params: &HashSet<&str>,
    params: &[super::decl::FunctionParam],
) -> Option<String> {
    // Check the resolved type on the expression.
    if let IrType::Generic(ref name) = expr.ty
        && type_params.contains(name.as_str())
    {
        return Some(name.clone());
    }

    // Also check if it's a Var referencing a param whose type is Generic.
    if let IrExprKind::Var { name, .. } = &expr.kind {
        for p in params {
            if &p.name == name
                && let IrType::Generic(ref tp_name) = p.ty
                && type_params.contains(tp_name.as_str())
            {
                return Some(tp_name.clone());
            }
        }
    }

    None
}

/// Map a binary operator to the required trait bound on the type parameter.
fn binop_to_trait_bound(op: &BinOp, tp_name: &str) -> Option<IrTraitBound> {
    match op {
        // Comparison
        BinOp::Eq | BinOp::Ne => Some(IrTraitBound::simple(tb::PARTIAL_EQ)),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Some(IrTraitBound::simple(tb::PARTIAL_ORD)),

        // Arithmetic
        BinOp::Add => Some(IrTraitBound::with_output(tb::ADD, IrType::Generic(tp_name.to_string()))),
        BinOp::Sub => Some(IrTraitBound::with_output(tb::SUB, IrType::Generic(tp_name.to_string()))),
        BinOp::Mul => Some(IrTraitBound::with_output(tb::MUL, IrType::Generic(tp_name.to_string()))),
        BinOp::Div => Some(IrTraitBound::with_output(tb::DIV, IrType::Generic(tp_name.to_string()))),
        BinOp::Mod => Some(IrTraitBound::with_output(tb::REM, IrType::Generic(tp_name.to_string()))),

        // Logical, bitwise, etc. — no trait bound inferred for these.
        BinOp::FloorDiv
        | BinOp::Pow
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => None,
    }
}

/// Add a trait bound to a type parameter, avoiding duplicates.
fn add_bound(bounds_map: &mut HashMap<String, Vec<IrTraitBound>>, tp_name: &str, bound: IrTraitBound) {
    let bounds = bounds_map.entry(tp_name.to_string()).or_default();
    if !bounds.contains(&bound) {
        bounds.push(bound);
    }
}

/// Remove duplicate bounds (by trait path).
fn deduplicate_bounds(bounds: Vec<IrTraitBound>) -> Vec<IrTraitBound> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for bound in bounds {
        if seen.insert(bound.trait_path.clone()) {
            result.push(bound);
        }
    }
    result
}

/// Collect calls to generic functions and their type argument mappings.
///
/// Returns a list of (callee name, type arg mapping) pairs. Each mapping connects the callee's type parameter names to
/// the caller's type parameter names when the argument is a direct type parameter pass-through.
fn collect_called_generic_functions(
    func: &IrFunction,
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
) -> Vec<(String, HashMap<String, String>)> {
    let type_param_names: HashSet<&str> = func.type_params.iter().map(|tp| tp.name.as_str()).collect();
    let mut result = Vec::new();

    for stmt in &func.body {
        collect_calls_in_stmt(
            stmt,
            &type_param_names,
            &func.params,
            function_bounds,
            function_params,
            &mut result,
        );
    }

    result
}

/// Recursively collect generic function calls from a statement.
fn collect_calls_in_stmt(
    stmt: &IrStmt,
    type_params: &HashSet<&str>,
    params: &[FunctionParam],
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
    result: &mut Vec<(String, HashMap<String, String>)>,
) {
    let recurse_expr = |e: &IrExpr, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_expr(e, type_params, params, function_bounds, function_params, r);
    };
    let recurse_stmt = |s: &IrStmt, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_stmt(s, type_params, params, function_bounds, function_params, r);
    };

    match &stmt.kind {
        IrStmtKind::Expr(expr) => recurse_expr(expr, result),
        IrStmtKind::Let { value, .. } | IrStmtKind::Assign { value, .. } | IrStmtKind::CompoundAssign { value, .. } => {
            recurse_expr(value, result)
        }
        IrStmtKind::Return(Some(expr)) => recurse_expr(expr, result),
        IrStmtKind::Return(None) | IrStmtKind::Break(_) | IrStmtKind::Continue(_) => {}
        IrStmtKind::While { condition, body, .. } => {
            recurse_expr(condition, result);
            for s in body {
                recurse_stmt(s, result);
            }
        }
        IrStmtKind::For { iterable, body, .. } => {
            recurse_expr(iterable, result);
            for s in body {
                recurse_stmt(s, result);
            }
        }
        IrStmtKind::Loop { body, .. } => {
            for s in body {
                recurse_stmt(s, result);
            }
        }
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            recurse_expr(condition, result);
            for s in then_branch {
                recurse_stmt(s, result);
            }
            if let Some(else_stmts) = else_branch {
                for s in else_stmts {
                    recurse_stmt(s, result);
                }
            }
        }
        IrStmtKind::Match { scrutinee, arms } => {
            recurse_expr(scrutinee, result);
            for arm in arms {
                recurse_expr(&arm.body, result);
                if let Some(guard) = &arm.guard {
                    recurse_expr(guard, result);
                }
            }
        }
        IrStmtKind::Block(stmts) => {
            for s in stmts {
                recurse_stmt(s, result);
            }
        }
    }
}

/// Recursively collect generic function calls from an expression.
fn collect_calls_in_expr(
    expr: &IrExpr,
    type_params: &HashSet<&str>,
    params: &[FunctionParam],
    function_bounds: &HashMap<String, Vec<IrTypeParam>>,
    function_params: &HashMap<String, Vec<FunctionParam>>,
    result: &mut Vec<(String, HashMap<String, String>)>,
) {
    let recurse_expr = |e: &IrExpr, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_expr(e, type_params, params, function_bounds, function_params, r);
    };
    let recurse_stmt = |s: &IrStmt, r: &mut Vec<(String, HashMap<String, String>)>| {
        collect_calls_in_stmt(s, type_params, params, function_bounds, function_params, r);
    };

    match &expr.kind {
        IrExprKind::Call { func, args, .. } => {
            // ---- Check if the called function is a generic function we know about ----
            if let IrExprKind::Var { name, .. } = &func.kind
                && function_bounds.contains_key(name.as_str())
            {
                let mut mapping = HashMap::new();

                // Use the callee's parameter types to determine which type parameter each argument corresponds to.
                // Named arguments (`foo(b=x)`) are matched by name; positional arguments by index.
                if let Some(callee_params) = function_params.get(name.as_str()) {
                    for (i, arg) in args.iter().enumerate() {
                        if let Some(caller_tp) = expr_type_param_name(&arg.expr, type_params, params) {
                            // Resolve the callee parameter: by name if the arg is named, by position otherwise.
                            let callee_param = if let Some(arg_name) = &arg.name {
                                callee_params.iter().find(|p| &p.name == arg_name)
                            } else {
                                callee_params.get(i)
                            };
                            if let Some(cp) = callee_param
                                && let IrType::Generic(ref callee_tp_name) = cp.ty
                            {
                                mapping.insert(callee_tp_name.clone(), caller_tp);
                            }
                        }
                    }
                }

                if !mapping.is_empty() {
                    result.push((name.clone(), mapping));
                }
            }

            // Recurse.
            recurse_expr(func, result);
            for arg in args {
                recurse_expr(&arg.expr, result);
            }
        }
        IrExprKind::BinOp { left, right, .. } => {
            recurse_expr(left, result);
            recurse_expr(right, result);
        }
        IrExprKind::UnaryOp { operand, .. } => {
            recurse_expr(operand, result);
        }
        IrExprKind::MethodCall { receiver, args, .. } => {
            recurse_expr(receiver, result);
            for arg in args {
                recurse_expr(&arg.expr, result);
            }
        }
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            recurse_expr(receiver, result);
            for arg in args {
                recurse_expr(&arg.expr, result);
            }
        }
        IrExprKind::Format { parts } => {
            for part in parts {
                if let FormatPart::Expr(e) = part {
                    recurse_expr(e, result);
                }
            }
        }
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            recurse_expr(condition, result);
            recurse_expr(then_branch, result);
            if let Some(e) = else_branch {
                recurse_expr(e, result);
            }
        }
        IrExprKind::Block { stmts, value } => {
            for s in stmts {
                recurse_stmt(s, result);
            }
            if let Some(v) = value {
                recurse_expr(v, result);
            }
        }
        // Other expression kinds are not recursed into for transitive inference.
        // The primary call pattern (direct function calls) is covered above.
        _ => {}
    }
}

/// Propagate bounds from a callee to a caller using the type argument mapping.
///
/// Returns `true` if any new bounds were added.
fn propagate_transitive_bounds(
    caller_bounds: &mut [IrTypeParam],
    callee_bounds: &[IrTypeParam],
    type_arg_mapping: &HashMap<String, String>,
) -> bool {
    let mut changed = false;

    for callee_tp in callee_bounds {
        // Check if this callee type param is mapped to a caller type param.
        if let Some(caller_tp_name) = type_arg_mapping.get(&callee_tp.name) {
            // Find the corresponding caller type param.
            if let Some(caller_tp) = caller_bounds.iter_mut().find(|tp| &tp.name == caller_tp_name) {
                for bound in &callee_tp.bounds {
                    if !caller_tp.bounds.contains(bound) {
                        caller_tp.bounds.push(bound.clone());
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}
