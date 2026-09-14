//! Free-variable analysis for closure and generator bodies: which names a deferred body captures.

use super::*;

/// Determine every lexical free variable used after a generator expression's first source, in first-occurrence
/// order. The initial source is evaluated before constructing [`bir::Rvalue::Generator`] and therefore is not a
/// deferred capture. Each `for` pattern becomes bound only after its own source expression has been visited, so a
/// later source/filter/element sees every preceding clause binding but not a name it introduces itself.
pub(super) fn free_vars_in_generator_deferred_body(generator: &ast::GeneratorExpr) -> Vec<String> {
    let Some((first, remaining)) = generator.clauses.split_first() else {
        return Vec::new();
    };
    let mut bound = HashSet::new();
    if let ast::ComprehensionClause::For { pattern, .. } = first {
        bind_pattern_names(&pattern.node, &mut bound);
    }
    let mut free = Vec::new();
    for clause in remaining {
        match clause {
            ast::ComprehensionClause::For { pattern, iter } => {
                collect_free_vars_in_expr(&iter.node, &mut bound, &mut free);
                bind_pattern_names(&pattern.node, &mut bound);
            }
            ast::ComprehensionClause::If(condition) => {
                collect_free_vars_in_expr(&condition.node, &mut bound, &mut free);
            }
        }
    }
    collect_free_vars_in_expr(&generator.expr.node, &mut bound, &mut free);
    free
}
/// Determine every free variable a closure literal's body reads from its enclosing scope, in first-occurrence
/// source order, given the closure's own declared parameters as the initial bound set. A "free variable" is any
/// `Ident` read the closure body itself does not bind -- exactly the set [`BodyBuilder::lower_closure`] must
/// capture before lowering the body, so each one gets its own explicit Duckborrower read at the point the closure
/// is constructed (see this module's docs on why Body IR cannot rely on a target backend's own closure syntax to
/// auto-capture the way the existing Rust-emission backend does).
pub(super) fn free_vars_in_closure_body(
    params: &[ast::Spanned<ast::Param>],
    body: &ast::Spanned<ast::Expr>,
) -> Vec<String> {
    let mut bound: HashSet<String> = params.iter().map(|p| p.node.name.clone()).collect();
    let mut free = Vec::new();
    collect_free_vars_in_expr(&body.node, &mut bound, &mut free);
    free
}
/// Record `name` in `free` (in first-occurrence order, deduplicated) unless it is already in `bound`.
pub(super) fn push_free(name: &str, bound: &HashSet<String>, free: &mut Vec<String>) {
    if !bound.contains(name) && !free.iter().any(|existing| existing == name) {
        free.push(name.to_string());
    }
}
/// Collect every name `pattern` binds into `bound`, recursing into every sub-pattern shape.
///
/// Used by [`collect_free_vars_in_expr`] to exclude a pattern's own bound names from the free variables an
/// enclosing closure must capture, for every construct that binds through a pattern: `match` arms, `for` loops,
/// comprehension/generator `for` clauses, and `if let`/`while let` conditions. A single recursive walk serves all
/// of them because a `for` pattern can now bind more than one name too (#1125) -- a flat "only a plain
/// [`ast::Pattern::Binding`] binds here" walk would leave a destructured loop binding looking free, and an
/// enclosing closure would wrongly capture it.
///
/// This mirrors [`BodyBuilder::lower_match_pattern`]'s and [`BodyBuilder::bind_for_pattern_fields`]' binding walks
/// in spirit, though it only needs the names, not the locals/ownership facts those walks build.
pub(super) fn bind_pattern_names(pattern: &ast::Pattern, bound: &mut HashSet<String>) {
    match pattern {
        ast::Pattern::Wildcard | ast::Pattern::Literal(_) => {}
        ast::Pattern::Binding(name) => {
            bound.insert(name.clone());
        }
        ast::Pattern::Tuple(items) => {
            for item in items {
                bind_pattern_names(&item.node, bound);
            }
        }
        ast::Pattern::Constructor(_, args) => {
            for arg in args {
                match arg {
                    ast::PatternArg::Positional(pat) | ast::PatternArg::Named(_, pat) => {
                        bind_pattern_names(&pat.node, bound);
                    }
                }
            }
        }
        ast::Pattern::Group(inner) => bind_pattern_names(&inner.node, bound),
        ast::Pattern::Or(items) => {
            for item in items {
                bind_pattern_names(&item.node, bound);
            }
        }
    }
}
/// Recursively collect free variables from an expression, given the names already bound at this point in `bound`.
/// Constructs that introduce their own bindings for a sub-expression (comprehension/`for`-clause patterns, nested
/// closures' own parameters, or a nested expression-position `if`/`loop`'s own statement-block bindings) extend a
/// *cloned* copy of `bound` before recursing into that sub-expression, so a binding introduced in one branch never
/// leaks into a sibling branch or back out to the caller -- unlike [`BodyBuilder`]'s own flat `self.bindings` map,
/// which this analysis runs entirely independently of (see [`free_vars_in_closure_body`]'s docs).
pub(super) fn collect_free_vars_in_expr(expr: &ast::Expr, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match expr {
        ast::Expr::Ident(name) => push_free(name, bound, free),
        ast::Expr::Binary(l, _, r) => {
            collect_free_vars_in_expr(&l.node, bound, free);
            collect_free_vars_in_expr(&r.node, bound, free);
        }
        ast::Expr::Unary(_, e) | ast::Expr::Paren(e) | ast::Expr::Try(e) => {
            collect_free_vars_in_expr(&e.node, bound, free)
        }
        ast::Expr::Call(callee, _, args) => {
            collect_free_vars_in_expr(&callee.node, bound, free);
            for arg in args {
                collect_free_vars_in_call_arg(arg, bound, free);
            }
        }
        ast::Expr::MethodCall(recv, _, _, args) => {
            collect_free_vars_in_expr(&recv.node, bound, free);
            for arg in args {
                collect_free_vars_in_call_arg(arg, bound, free);
            }
        }
        ast::Expr::Field(e, _) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Expr::Index(e, idx) => {
            collect_free_vars_in_expr(&e.node, bound, free);
            collect_free_vars_in_expr(&idx.node, bound, free);
        }
        ast::Expr::Slice(base, slice) => {
            collect_free_vars_in_expr(&base.node, bound, free);
            for component in [&slice.start, &slice.end, &slice.step].into_iter().flatten() {
                collect_free_vars_in_expr(&component.node, bound, free);
            }
        }
        ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
            for item in items {
                collect_free_vars_in_expr(&item.node, bound, free);
            }
        }
        ast::Expr::List(entries) => {
            for entry in entries {
                match entry {
                    ast::ListEntry::Element(e) | ast::ListEntry::Spread(e) => {
                        collect_free_vars_in_expr(&e.node, bound, free)
                    }
                }
            }
        }
        ast::Expr::Dict(entries) => {
            for entry in entries {
                match entry {
                    ast::DictEntry::Pair(k, v) => {
                        collect_free_vars_in_expr(&k.node, bound, free);
                        collect_free_vars_in_expr(&v.node, bound, free);
                    }
                    ast::DictEntry::Spread(e) => collect_free_vars_in_expr(&e.node, bound, free),
                }
            }
        }
        ast::Expr::Constructor(_, args) => {
            for arg in args {
                collect_free_vars_in_call_arg(arg, bound, free);
            }
        }
        ast::Expr::Range { start, end, .. } => {
            collect_free_vars_in_expr(&start.node, bound, free);
            collect_free_vars_in_expr(&end.node, bound, free);
        }
        ast::Expr::FString(parts) => {
            for part in parts {
                if let ast::FStringPart::Expr { expr, .. } = part {
                    collect_free_vars_in_expr(&expr.node, bound, free);
                }
            }
        }
        ast::Expr::If(if_expr) => {
            collect_free_vars_in_expr(&if_expr.condition.node, bound, free);
            let mut then_bound = bound.clone();
            collect_free_vars_in_stmts(&if_expr.then_body, &mut then_bound, free);
            if let Some(else_body) = &if_expr.else_body {
                let mut else_bound = bound.clone();
                collect_free_vars_in_stmts(else_body, &mut else_bound, free);
            }
        }
        ast::Expr::Loop(loop_expr) => {
            let mut loop_bound = bound.clone();
            collect_free_vars_in_stmts(&loop_expr.body, &mut loop_bound, free);
        }
        ast::Expr::ListComp(comp) => {
            collect_free_vars_in_expr(&comp.iter.node, bound, free);
            let mut inner_bound = bound.clone();
            bind_pattern_names(&comp.pattern.node, &mut inner_bound);
            if let Some(filter) = &comp.filter {
                collect_free_vars_in_expr(&filter.node, &mut inner_bound, free);
            }
            collect_free_vars_in_expr(&comp.expr.node, &mut inner_bound, free);
        }
        ast::Expr::DictComp(comp) => {
            collect_free_vars_in_expr(&comp.iter.node, bound, free);
            let mut inner_bound = bound.clone();
            bind_pattern_names(&comp.pattern.node, &mut inner_bound);
            if let Some(filter) = &comp.filter {
                collect_free_vars_in_expr(&filter.node, &mut inner_bound, free);
            }
            collect_free_vars_in_expr(&comp.key.node, &mut inner_bound, free);
            collect_free_vars_in_expr(&comp.value.node, &mut inner_bound, free);
        }
        ast::Expr::Generator(generator) => {
            let mut inner_bound = bound.clone();
            for clause in &generator.clauses {
                match clause {
                    ast::ComprehensionClause::For { pattern, iter } => {
                        collect_free_vars_in_expr(&iter.node, &mut inner_bound, free);
                        bind_pattern_names(&pattern.node, &mut inner_bound);
                    }
                    ast::ComprehensionClause::If(cond) => collect_free_vars_in_expr(&cond.node, &mut inner_bound, free),
                }
            }
            collect_free_vars_in_expr(&generator.expr.node, &mut inner_bound, free);
        }
        ast::Expr::Closure(params, body) => {
            let mut inner_bound = bound.clone();
            for param in params {
                inner_bound.insert(param.node.name.clone());
            }
            collect_free_vars_in_expr(&body.node, &mut inner_bound, free);
        }
        ast::Expr::Partial(partial) => {
            collect_free_vars_in_expr(&partial.target.node, bound, free);
            for arg in &partial.args {
                collect_free_vars_in_expr(&arg.value.node, bound, free);
            }
        }
        // Mirrors `count_reads_in_expr`'s `Yield` arm: a yielded value is an ordinary nested expression for
        // free-variable purposes, so a name it reads from an enclosing closure scope must still be captured.
        ast::Expr::Yield(Some(value)) => collect_free_vars_in_expr(&value.node, bound, free),
        // The scrutinee is read in the enclosing scope like any other sub-expression. Each arm gets its own
        // *cloned* `bound` set (matching the `If`/`Loop` arms above) extended with that arm's own pattern-bound
        // names before walking its guard and body, so one arm's bindings never leak into a sibling arm or shadow an
        // outer free variable of the same name.
        ast::Expr::Match(subject, arms) => {
            collect_free_vars_in_expr(&subject.node, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                bind_pattern_names(&arm.node.pattern.node, &mut arm_bound);
                if let Some(guard) = &arm.node.guard {
                    collect_free_vars_in_expr(&guard.node, &mut arm_bound, free);
                }
                match &arm.node.body {
                    ast::MatchBody::Expr(e) => collect_free_vars_in_expr(&e.node, &mut arm_bound, free),
                    ast::MatchBody::Block(stmts) => collect_free_vars_in_stmts(stmts, &mut arm_bound, free),
                }
            }
        }
        _ => {}
    }
}
/// Collect free variables from one call argument's expression, regardless of whether it is positional, named, or an
/// unpack -- matching [`count_reads_in_call_arg`]'s own "count the expression either way" stance, even though
/// [`BodyBuilder::lower_positional_args`] itself rejects named/unpack arguments during real lowering.
pub(super) fn collect_free_vars_in_call_arg(arg: &ast::CallArg, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match arg {
        ast::CallArg::Positional(e)
        | ast::CallArg::Named(_, e)
        | ast::CallArg::PositionalUnpack(e)
        | ast::CallArg::KeywordUnpack(e) => collect_free_vars_in_expr(&e.node, bound, free),
    }
}
/// Collect free variables from an `if`/`while` condition, including the value expression of a `Condition::Let`
/// pattern condition (even though v0 lowering does not model `if let`/`while let` themselves -- see
/// [`BodyBuilder::lower_if`]/[`BodyBuilder::lower_while`]) -- a pattern-bound name still shadows an outer name of
/// the same spelling for anything nested inside the branch this condition gates, so it is bound defensively here
/// even though the branch itself lowers to `Unsupported`.
pub(super) fn collect_free_vars_in_condition(
    cond: &ast::Condition,
    bound: &mut HashSet<String>,
    free: &mut Vec<String>,
) {
    match cond {
        ast::Condition::Expr(e) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Condition::Let { pattern, value } => {
            collect_free_vars_in_expr(&value.node, bound, free);
            bind_pattern_names(&pattern.node, bound);
        }
    }
}

/// Collect free variables from a statement block in source order, threading a progressively-extended `bound` set
/// through each statement so a binding one statement introduces (`let`, `for`, tuple unpack, ...) is visible to
/// every later statement in the *same* block, matching ordinary lexical scoping -- and, symmetrically, does not
/// leak into a sibling block (an `if`'s `else` body, for instance), since callers always pass a freshly cloned
/// `bound` per block (see [`collect_free_vars_in_expr`]'s `If`/`Loop` arms).
pub(super) fn collect_free_vars_in_stmts(
    stmts: &[ast::Spanned<ast::Statement>],
    bound: &mut HashSet<String>,
    free: &mut Vec<String>,
) {
    for stmt in stmts {
        collect_free_vars_in_stmt(&stmt.node, bound, free);
    }
}
/// Collect free variables from one statement, recursing into every statement kind [`BodyBuilder`]'s own lowering
/// walks (see this module's module-level docs for the covered subset) and extending `bound` wherever that statement
/// introduces a new binding for the remainder of its enclosing block. Statement kinds outside v0's lowered subset
/// are not walked and neither read nor bind anything this analysis needs to know about.
pub(super) fn collect_free_vars_in_stmt(stmt: &ast::Statement, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match stmt {
        ast::Statement::Assignment(a) => {
            collect_free_vars_in_expr(&a.value.node, bound, free);
            bound.insert(a.name.clone());
        }
        ast::Statement::FieldAssignment(fa) => {
            collect_free_vars_in_expr(&fa.object.node, bound, free);
            collect_free_vars_in_expr(&fa.value.node, bound, free);
        }
        ast::Statement::IndexAssignment(ia) => {
            collect_free_vars_in_expr(&ia.object.node, bound, free);
            collect_free_vars_in_expr(&ia.index.node, bound, free);
            collect_free_vars_in_expr(&ia.value.node, bound, free);
        }
        ast::Statement::CompoundAssignment(ca) => {
            // A compound assignment target must already exist, so it is a read of whatever bound it (an outer
            // capture, if this statement lives inside a closure body and `ca.name` was never rebound locally), not
            // a fresh binding -- see `Self::lower_partial`'s docs for the known limitation this implies for
            // mutating a captured variable from inside a closure.
            push_free(&ca.name, bound, free);
            collect_free_vars_in_expr(&ca.value.node, bound, free);
        }
        ast::Statement::TupleUnpack(tu) => {
            collect_free_vars_in_expr(&tu.value.node, bound, free);
            for name in &tu.names {
                bound.insert(name.clone());
            }
        }
        ast::Statement::TupleAssign(ta) => {
            for target in &ta.targets {
                collect_free_vars_in_expr(&target.node, bound, free);
            }
            collect_free_vars_in_expr(&ta.value.node, bound, free);
        }
        ast::Statement::ChainedAssignment(ca) => {
            collect_free_vars_in_expr(&ca.value.node, bound, free);
            for name in &ca.targets {
                bound.insert(name.clone());
            }
        }
        ast::Statement::Return(Some(e)) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Statement::Return(None) => {}
        ast::Statement::If(if_stmt) => {
            collect_free_vars_in_condition(&if_stmt.condition, bound, free);
            let mut then_bound = bound.clone();
            collect_free_vars_in_stmts(&if_stmt.then_body, &mut then_bound, free);
            for (cond, body) in &if_stmt.elif_branches {
                collect_free_vars_in_expr(&cond.node, bound, free);
                let mut elif_bound = bound.clone();
                collect_free_vars_in_stmts(body, &mut elif_bound, free);
            }
            if let Some(else_body) = &if_stmt.else_body {
                let mut else_bound = bound.clone();
                collect_free_vars_in_stmts(else_body, &mut else_bound, free);
            }
        }
        ast::Statement::While(w) => {
            collect_free_vars_in_condition(&w.condition, bound, free);
            let mut loop_bound = bound.clone();
            collect_free_vars_in_stmts(&w.body, &mut loop_bound, free);
        }
        ast::Statement::For(f) => {
            collect_free_vars_in_expr(&f.iter.node, bound, free);
            let mut loop_bound = bound.clone();
            bind_pattern_names(&f.pattern.node, &mut loop_bound);
            collect_free_vars_in_stmts(&f.body, &mut loop_bound, free);
        }
        ast::Statement::Expr(e) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Statement::Assert(a) => {
            // All three assertion forms lower their payload expression (#1167), and the pattern form additionally
            // binds its names for the rest of the block -- so a closure containing one must capture what the
            // payload reads, and must not mistake a name the pattern itself bound for a capture.
            match &a.kind {
                ast::AssertKind::Condition(e) => collect_free_vars_in_expr(&e.node, bound, free),
                ast::AssertKind::Raises { call, .. } => collect_free_vars_in_expr(&call.node, bound, free),
                ast::AssertKind::IsPattern { value, pattern } => {
                    collect_free_vars_in_expr(&value.node, bound, free);
                    bind_pattern_names(&pattern.node, bound);
                }
            }
            if let Some(message) = &a.message {
                collect_free_vars_in_expr(&message.node, bound, free);
            }
        }
        ast::Statement::Break(Some(e)) => collect_free_vars_in_expr(&e.node, bound, free),
        _ => {}
    }
}
