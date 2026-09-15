//! Read-count analysis: how many times a name is read, which seeds each binding's last-use countdown.

use super::*;

/// Count `name` occurrences across a tail of comprehension/generator clauses, for seeding a `for`-clause binding's
/// last-use countdown alongside [`ComprehensionTerminal::count_reads`] (see
/// [`BodyBuilder::lower_comprehension_clauses`]).
pub(super) fn count_reads_in_comprehension_clauses(name: &str, clauses: &[ast::ComprehensionClause]) -> usize {
    clauses
        .iter()
        .map(|clause| match clause {
            ast::ComprehensionClause::For { iter, .. } => count_reads_in_expr(name, &iter.node),
            ast::ComprehensionClause::If(cond) => count_reads_in_expr(name, &cond.node),
        })
        .sum()
}
/// Count textual `Ident(name)` occurrences reachable from `stmts`, restricted to the same statement/expression
/// subset [`BodyBuilder`] actually lowers. This seeds a local's last-use countdown (see
/// [`BodyBuilder::declare_new_local`]).
///
/// This is a **textual, source-order over-approximation**, not dynamic dataflow: it does not special-case shadowing
/// (a later redeclaration of the same name still contributes to this count) and it counts occurrences across all
/// branches of a conditional rather than only the branch that will execute. Both simplifications only ever make the
/// count too high, which biases the resulting ownership fact toward `Clone`/`Borrow` instead of `Move` — never the
/// reverse — so it cannot produce an unsound move.
pub(super) fn count_reads_in_stmts(name: &str, stmts: &[ast::Spanned<ast::Statement>]) -> usize {
    stmts.iter().map(|stmt| count_reads_in_stmt(name, &stmt.node)).sum()
}
/// Count `name` occurrences reachable from one statement, recursing into every branch of a conditional/loop rather
/// than only the branch that will execute — part of [`count_reads_in_stmts`]'s documented over-approximation.
/// Statement kinds outside v0's lowered subset are not walked and contribute zero (they cannot themselves bind or
/// read `name` in a way v0's lowering will ever observe).
pub(super) fn count_reads_in_stmt(name: &str, stmt: &ast::Statement) -> usize {
    match stmt {
        ast::Statement::Assignment(a) => count_reads_in_expr(name, &a.value.node),
        ast::Statement::FieldAssignment(fa) => {
            count_reads_in_expr(name, &fa.object.node) + count_reads_in_expr(name, &fa.value.node)
        }
        ast::Statement::IndexAssignment(ia) => {
            count_reads_in_expr(name, &ia.object.node)
                + count_reads_in_expr(name, &ia.index.node)
                + count_reads_in_expr(name, &ia.value.node)
        }
        ast::Statement::CompoundAssignment(ca) => {
            usize::from(ca.name == name) + count_reads_in_expr(name, &ca.value.node)
        }
        ast::Statement::TupleUnpack(tu) => count_reads_in_expr(name, &tu.value.node),
        ast::Statement::TupleAssign(ta) => {
            ta.targets
                .iter()
                .map(|t| count_reads_in_expr(name, &t.node))
                .sum::<usize>()
                + count_reads_in_expr(name, &ta.value.node)
        }
        ast::Statement::ChainedAssignment(ca) => count_reads_in_expr(name, &ca.value.node),
        ast::Statement::Return(Some(e)) => count_reads_in_expr(name, &e.node),
        ast::Statement::Return(None) => 0,
        ast::Statement::If(if_stmt) => {
            let mut total = count_reads_in_condition(name, &if_stmt.condition);
            total += count_reads_in_stmts(name, &if_stmt.then_body);
            for (cond, body) in &if_stmt.elif_branches {
                total += count_reads_in_expr(name, &cond.node);
                total += count_reads_in_stmts(name, body);
            }
            if let Some(else_body) = &if_stmt.else_body {
                total += count_reads_in_stmts(name, else_body);
            }
            total
        }
        ast::Statement::While(w) => count_reads_in_condition(name, &w.condition) + count_reads_in_stmts(name, &w.body),
        ast::Statement::For(f) => count_reads_in_expr(name, &f.iter.node) + count_reads_in_stmts(name, &f.body),
        ast::Statement::Expr(e) => count_reads_in_expr(name, &e.node),
        ast::Statement::Assert(a) => {
            // Every assertion form lowers its own payload expression (#1167), so all three are counted here.
            // Missing one would let an earlier read of the same name be selected as its last use and moved out
            // from under the assertion, which is the one direction this approximation must never take.
            let mut total = match &a.kind {
                ast::AssertKind::Condition(e) => count_reads_in_expr(name, &e.node),
                ast::AssertKind::IsPattern { value, .. } => count_reads_in_expr(name, &value.node),
                ast::AssertKind::Raises { call, .. } => count_reads_in_expr(name, &call.node),
            };
            total += a
                .message
                .as_ref()
                .map(|m| count_reads_in_expr(name, &m.node))
                .unwrap_or(0);
            total
        }
        ast::Statement::Break(Some(e)) => count_reads_in_expr(name, &e.node),
        _ => 0,
    }
}
/// Count `name` occurrences in an `if`/`while` condition, including the value expression of a `Condition::Let`
/// pattern condition (even though v0 lowering does not model `if let`/`while let` themselves — see
/// [`BodyBuilder::lower_if`]/[`BodyBuilder::lower_while`] — so the read-count approximation stays an
/// over-approximation rather than silently under-counting).
pub(super) fn count_reads_in_condition(name: &str, cond: &ast::Condition) -> usize {
    match cond {
        ast::Condition::Expr(e) => count_reads_in_expr(name, &e.node),
        ast::Condition::Let { value, .. } => count_reads_in_expr(name, &value.node),
    }
}
/// Count `name` occurrences reachable from one expression, recursing into every expression kind v0's lowering
/// itself walks (see this module's module-level docs for the covered subset). Expression kinds outside that subset
/// contribute zero, consistent with [`count_reads_in_stmts`]'s "restricted to the same subset `BodyBuilder` actually
/// lowers" scope.
pub(super) fn count_reads_in_expr(name: &str, expr: &ast::Expr) -> usize {
    match expr {
        ast::Expr::Ident(id) => usize::from(id == name),
        ast::Expr::Binary(l, _, r) => count_reads_in_expr(name, &l.node) + count_reads_in_expr(name, &r.node),
        ast::Expr::Unary(_, e) => count_reads_in_expr(name, &e.node),
        ast::Expr::Call(callee, _, args) => {
            count_reads_in_expr(name, &callee.node)
                + args.iter().map(|a| count_reads_in_call_arg(name, a)).sum::<usize>()
        }
        ast::Expr::MethodCall(recv, _, _, args) => {
            count_reads_in_expr(name, &recv.node) + args.iter().map(|a| count_reads_in_call_arg(name, a)).sum::<usize>()
        }
        ast::Expr::Field(e, _) => count_reads_in_expr(name, &e.node),
        ast::Expr::Index(e, idx) => count_reads_in_expr(name, &e.node) + count_reads_in_expr(name, &idx.node),
        ast::Expr::Slice(base, slice) => {
            count_reads_in_expr(name, &base.node)
                + slice
                    .start
                    .as_ref()
                    .map(|e| count_reads_in_expr(name, &e.node))
                    .unwrap_or(0)
                + slice
                    .end
                    .as_ref()
                    .map(|e| count_reads_in_expr(name, &e.node))
                    .unwrap_or(0)
                + slice
                    .step
                    .as_ref()
                    .map(|e| count_reads_in_expr(name, &e.node))
                    .unwrap_or(0)
        }
        ast::Expr::Paren(e) | ast::Expr::Try(e) => count_reads_in_expr(name, &e.node),
        ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
            items.iter().map(|i| count_reads_in_expr(name, &i.node)).sum()
        }
        ast::Expr::List(entries) => entries
            .iter()
            .map(|entry| match entry {
                ast::ListEntry::Element(e) | ast::ListEntry::Spread(e) => count_reads_in_expr(name, &e.node),
            })
            .sum(),
        ast::Expr::Dict(entries) => entries
            .iter()
            .map(|entry| match entry {
                ast::DictEntry::Pair(k, v) => count_reads_in_expr(name, &k.node) + count_reads_in_expr(name, &v.node),
                ast::DictEntry::Spread(e) => count_reads_in_expr(name, &e.node),
            })
            .sum(),
        ast::Expr::Constructor(_, args) => args.iter().map(|a| count_reads_in_call_arg(name, a)).sum(),
        ast::Expr::Range { start, end, .. } => {
            count_reads_in_expr(name, &start.node) + count_reads_in_expr(name, &end.node)
        }
        ast::Expr::If(if_expr) => {
            count_reads_in_expr(name, &if_expr.condition.node)
                + count_reads_in_stmts(name, &if_expr.then_body)
                + if_expr
                    .else_body
                    .as_ref()
                    .map(|body| count_reads_in_stmts(name, body))
                    .unwrap_or(0)
        }
        ast::Expr::Loop(loop_expr) => count_reads_in_stmts(name, &loop_expr.body),
        ast::Expr::FString(parts) => parts
            .iter()
            .map(|part| match part {
                ast::FStringPart::Literal(_) => 0,
                ast::FStringPart::Expr { expr, .. } => count_reads_in_expr(name, &expr.node),
            })
            .sum(),
        ast::Expr::ListComp(comp) => {
            count_reads_in_expr(name, &comp.iter.node)
                + comp
                    .filter
                    .as_ref()
                    .map(|f| count_reads_in_expr(name, &f.node))
                    .unwrap_or(0)
                + count_reads_in_expr(name, &comp.expr.node)
        }
        ast::Expr::DictComp(comp) => {
            count_reads_in_expr(name, &comp.iter.node)
                + comp
                    .filter
                    .as_ref()
                    .map(|f| count_reads_in_expr(name, &f.node))
                    .unwrap_or(0)
                + count_reads_in_expr(name, &comp.key.node)
                + count_reads_in_expr(name, &comp.value.node)
        }
        ast::Expr::Generator(generator) => {
            count_reads_in_comprehension_clauses(name, &generator.clauses)
                + count_reads_in_expr(name, &generator.expr.node)
        }
        ast::Expr::Closure(params, body) => {
            // `BodyBuilder::lower_closure` reads a captured free variable exactly once at the closure-creation
            // site, however many times the closure body itself uses it afterward (subsequent uses read the
            // closure's own captured-binding local, not the outer one this count seeds) -- so this contributes at
            // most 1, not the raw in-body occurrence count. A name shadowed by the closure's own parameter is never
            // captured at all and so contributes 0, regardless of how many times the body uses its own parameter.
            if params.iter().any(|p| p.node.name == name) {
                0
            } else {
                usize::from(count_reads_in_expr(name, &body.node) > 0)
            }
        }
        ast::Expr::Partial(partial) => {
            // Unlike a closure's captures, a partial callable's preset values are lowered as ordinary sub-expression
            // reads (see `BodyBuilder::lower_partial`), not deduplicated per free-variable name, so this counts them
            // plainly like any other nested expression.
            count_reads_in_expr(name, &partial.target.node)
                + partial
                    .args
                    .iter()
                    .map(|a| count_reads_in_expr(name, &a.value.node))
                    .sum::<usize>()
        }
        // `BodyBuilder::lower_yield` lowers a yielded value through the same `lower_expr_to_operand` path as any
        // other statement's operand, so a name read inside `yield value` must be counted here too -- otherwise it
        // would be undercounted for last-use purposes, the same soundness gap #1101's f-string bucket found and
        // fixed for `count_reads_in_expr`'s `FString` arm.
        ast::Expr::Yield(value) => value.as_ref().map_or(0, |v| count_reads_in_expr(name, &v.node)),
        // Same soundness class as the `Yield`/`FString` arms above: a `match` scrutinee, guard, or arm body is
        // lowered through the ordinary expression/statement paths (`BodyBuilder::lower_match`), so a read of `name`
        // reachable inside any of them must be counted here too. Unlike `collect_free_vars_in_expr`'s `Match` arm,
        // this does not need to exclude an arm's own pattern-bound names from the count: this function is a coarse,
        // source-order over-approximation by design (see its own docs), and over-counting only ever biases the
        // resulting ownership fact toward `Clone`/`Borrow` rather than `Move` -- never unsound.
        ast::Expr::Match(subject, arms) => {
            count_reads_in_expr(name, &subject.node)
                + arms
                    .iter()
                    .map(|arm| count_reads_in_match_arm(name, &arm.node))
                    .sum::<usize>()
        }
        _ => 0,
    }
}
/// Count `name` occurrences reachable from one `match` arm's guard and body, for seeding a pattern-bound local's
/// last-use countdown the same way [`count_reads_in_stmts`] seeds an ordinary binding's -- see
/// [`BodyBuilder::lower_match_pattern`]. Also reused by [`count_reads_in_expr`]'s own `Match` arm so both counting
/// paths agree on what "a read inside this arm" means.
pub(super) fn count_reads_in_match_arm(name: &str, arm: &ast::MatchArm) -> usize {
    let guard_reads = arm.guard.as_ref().map_or(0, |g| count_reads_in_expr(name, &g.node));
    let body_reads = match &arm.body {
        ast::MatchBody::Expr(e) => count_reads_in_expr(name, &e.node),
        ast::MatchBody::Block(stmts) => count_reads_in_stmts(name, stmts),
    };
    guard_reads + body_reads
}
/// Count a generator capture's deferred reads after the first source. This intentionally remains a conservative
/// source-order over-approximation, like [`count_reads_in_expr`]: a later pattern can shadow the same spelling and
/// leave this count high, which selects a clone rather than an unsound move in the generator-local body.
pub(super) fn count_reads_in_generator_deferred_body(name: &str, generator: &ast::GeneratorExpr) -> usize {
    let Some((_, remaining)) = generator.clauses.split_first() else {
        return 0;
    };
    count_reads_in_comprehension_clauses(name, remaining) + count_reads_in_expr(name, &generator.expr.node)
}
/// Count `name` occurrences in one call argument's expression, regardless of whether the argument is positional,
/// named, or an unpack — the read-count approximation counts the expression either way even though
/// [`BodyBuilder::lower_positional_args`] itself rejects named/unpack arguments during real lowering.
pub(super) fn count_reads_in_call_arg(name: &str, arg: &ast::CallArg) -> usize {
    match arg {
        ast::CallArg::Positional(e)
        | ast::CallArg::Named(_, e)
        | ast::CallArg::PositionalUnpack(e)
        | ast::CallArg::KeywordUnpack(e) => count_reads_in_expr(name, &e.node),
    }
}
