//! Static writes performed by static initializers.

use super::*;

impl TypeChecker {
    /// Validate RFC 052 "no static assignment in static initializers" by walking expression-driven call graphs.
    ///
    /// This intentionally follows local helper function calls reachable from the initializer expression so hidden
    /// `static` writes in helper bodies are rejected before runtime emission.
    pub(super) fn collect_static_initializer_static_writes_from_expr(
        &mut self,
        expr: &Spanned<Expr>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &expr.node {
            Expr::Literal(_) | Expr::Ident(_) | Expr::SelfExpr | Expr::Yield(None) => {}
            Expr::Binary(left, _, right) => {
                self.collect_static_initializer_static_writes_from_expr(left, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(right, current_static, visiting_functions);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
                self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions);
            }
            Expr::Call(func, _type_args, args) => {
                self.collect_static_initializer_static_writes_from_expr(func, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
                self.collect_static_initializer_local_function_writes(func, current_static, visiting_functions);
            }
            Expr::MethodCall(object, _, _type_args, args) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
            }
            Expr::Partial(partial) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &partial.target,
                    current_static,
                    visiting_functions,
                );
                for arg in &partial.args {
                    self.collect_static_initializer_static_writes_from_expr(
                        &arg.value,
                        current_static,
                        visiting_functions,
                    );
                }
            }
            Expr::Index(object, index) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(index, current_static, visiting_functions);
            }
            Expr::Slice(target, slice) => {
                self.collect_static_initializer_static_writes_from_expr(target, current_static, visiting_functions);
                if let Some(start) = &slice.start {
                    self.collect_static_initializer_static_writes_from_expr(start, current_static, visiting_functions);
                }
                if let Some(end) = &slice.end {
                    self.collect_static_initializer_static_writes_from_expr(end, current_static, visiting_functions);
                }
                if let Some(step) = &slice.step {
                    self.collect_static_initializer_static_writes_from_expr(step, current_static, visiting_functions);
                }
            }
            Expr::Field(object, _) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
            }
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_static_initializer_static_writes_from_expr(item, current_static, visiting_functions);
                }
            }
            Expr::List(items) => {
                self.collect_static_initializer_list_writes(items, current_static, visiting_functions);
            }
            Expr::Dict(items) => {
                self.collect_static_initializer_dict_writes(items, current_static, visiting_functions);
            }
            Expr::FString(parts) => {
                self.collect_static_initializer_fstring_writes(parts, current_static, visiting_functions);
            }
            Expr::Range { start, end, .. } => {
                self.collect_static_initializer_static_writes_from_expr(start, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(end, current_static, visiting_functions);
            }
            Expr::Constructor(_, args) => {
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_static_initializer_static_writes_from_expr(scrutinee, current_static, visiting_functions);
                self.collect_static_initializer_match_writes(arms, current_static, visiting_functions);
            }
            Expr::If(if_expr) => {
                self.collect_static_initializer_if_writes(if_expr, current_static, visiting_functions);
            }
            Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
                }
            }
            Expr::Generator(generator) => {
                self.collect_static_initializer_generator_writes(generator, current_static, visiting_functions);
            }
            Expr::ListComp(list_comp) => {
                self.collect_static_initializer_list_comp_writes(list_comp, current_static, visiting_functions);
            }
            Expr::DictComp(dict_comp) => {
                self.collect_static_initializer_dict_comp_writes(dict_comp, current_static, visiting_functions);
            }
            Expr::Closure(_, _) => {}
            Expr::Surface(surface) => {
                self.collect_static_initializer_surface_writes(&surface.payload, current_static, visiting_functions)
            }
            Expr::VocabBlock(block) => {
                for arg in &block.header_args {
                    self.collect_static_initializer_static_writes_from_expr(arg, current_static, visiting_functions);
                }
                for stmt in &block.body {
                    self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
                }
            }
            // A static write can only be written in ordinary Incan, so only a fragment's expression holes can
            // reach one. `holes` is the single authority on which expressions those are (RFC 081, #1022).
            Expr::Embedded(fragment) => {
                for hole in fragment.holes() {
                    self.collect_static_initializer_static_writes_from_expr(hole, current_static, visiting_functions);
                }
            }
        }
    }

    /// Follow a local direct helper once on the active call stack; closure creation alone does not execute its body.
    pub(super) fn collect_static_initializer_local_function_writes(
        &mut self,
        func: &Spanned<Expr>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        if let Expr::Ident(function_name) = &func.node
            && let Some(function_decl) = self.local_function_decls.get(function_name).cloned()
            && visiting_functions.insert(function_name.clone())
        {
            self.collect_static_initializer_body_writes(&function_decl.body, current_static, visiting_functions);
            visiting_functions.remove(function_name);
        }
    }

    /// Check statements in source order without changing the initializer call-stack cycle guard.
    pub(super) fn collect_static_initializer_body_writes(
        &mut self,
        body: &[Spanned<Statement>],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for stmt in body {
            self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
        }
    }

    /// Check dictionary keys, values and spreads in their written order.
    pub(super) fn collect_static_initializer_dict_writes(
        &mut self,
        items: &[DictEntry],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for item in items {
            match item {
                DictEntry::Pair(key, value) => {
                    self.collect_static_initializer_static_writes_from_expr(key, current_static, visiting_functions);
                    self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
                }
                DictEntry::Spread(value) => {
                    self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
                }
            }
        }
    }

    /// Check list elements and spread values in their written order.
    pub(super) fn collect_static_initializer_list_writes(
        &mut self,
        items: &[ListEntry],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for item in items {
            let (ListEntry::Element(value) | ListEntry::Spread(value)) = item;
            self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
        }
    }

    /// Check interpolated expressions while leaving literal format text outside the static-write analysis.
    pub(super) fn collect_static_initializer_fstring_writes(
        &mut self,
        parts: &[FStringPart],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for part in parts {
            if let FStringPart::Expr { expr, .. } = part {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
        }
    }

    /// Check a list comprehension's result, source and optional filter in the existing analysis order.
    pub(super) fn collect_static_initializer_list_comp_writes(
        &mut self,
        expr: &ListComp,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&expr.expr, current_static, visiting_functions);
        self.collect_static_initializer_static_writes_from_expr(&expr.iter, current_static, visiting_functions);
        if let Some(filter) = &expr.filter {
            self.collect_static_initializer_static_writes_from_expr(filter, current_static, visiting_functions);
        }
    }

    /// Check a dictionary comprehension's key, value, source and optional filter in the existing analysis order.
    pub(super) fn collect_static_initializer_dict_comp_writes(
        &mut self,
        expr: &DictComp,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&expr.key, current_static, visiting_functions);
        self.collect_static_initializer_static_writes_from_expr(&expr.value, current_static, visiting_functions);
        self.collect_static_initializer_static_writes_from_expr(&expr.iter, current_static, visiting_functions);
        if let Some(filter) = &expr.filter {
            self.collect_static_initializer_static_writes_from_expr(filter, current_static, visiting_functions);
        }
    }

    /// Check each match guard before its expression or statement body.
    pub(super) fn collect_static_initializer_match_writes(
        &mut self,
        arms: &[Spanned<MatchArm>],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arm in arms {
            if let Some(guard) = &arm.node.guard {
                self.collect_static_initializer_static_writes_from_expr(guard, current_static, visiting_functions);
            }
            match &arm.node.body {
                MatchBody::Expr(inner) => {
                    self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions)
                }
                MatchBody::Block(body) => {
                    self.collect_static_initializer_body_writes(body, current_static, visiting_functions)
                }
            }
        }
    }

    /// Check the condition and both branches of an initializer-reachable if expression.
    pub(super) fn collect_static_initializer_if_writes(
        &mut self,
        expr: &IfExpr,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&expr.condition, current_static, visiting_functions);
        self.collect_static_initializer_body_writes(&expr.then_body, current_static, visiting_functions);
        if let Some(body) = &expr.else_body {
            self.collect_static_initializer_body_writes(body, current_static, visiting_functions);
        }
    }

    /// Check the generator result followed by its source and filter clauses, preserving existing traversal order.
    pub(super) fn collect_static_initializer_generator_writes(
        &mut self,
        generator: &GeneratorExpr,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&generator.expr, current_static, visiting_functions);
        for clause in &generator.clauses {
            let expr = match clause {
                ComprehensionClause::For { iter, .. } => iter,
                ComprehensionClause::If(condition) => condition,
            };
            self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
        }
    }

    /// Check ordinary Incan slots within a surface payload, including race-arm bodies and scoped call arguments.
    pub(super) fn collect_static_initializer_surface_writes(
        &mut self,
        payload: &SurfaceExprPayload,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match payload {
            SurfaceExprPayload::PrefixUnary(inner) => {
                self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions)
            }
            SurfaceExprPayload::RaceFor(race) => {
                for arm in &race.arms {
                    self.collect_static_initializer_static_writes_from_expr(
                        &arm.awaitable,
                        current_static,
                        visiting_functions,
                    );
                    match &arm.body {
                        RaceForBody::Expr(expr) => self.collect_static_initializer_static_writes_from_expr(
                            expr,
                            current_static,
                            visiting_functions,
                        ),
                        RaceForBody::Block(body) => {
                            self.collect_static_initializer_body_writes(body, current_static, visiting_functions)
                        }
                    }
                }
            }
            SurfaceExprPayload::LeadingDotPath { .. } => {}
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.collect_static_initializer_static_writes_from_expr(left, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(right, current_static, visiting_functions);
            }
            SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions)
            }
        }
    }

    /// Recurse through call arguments while checking initializer-reachable static writes.
    pub(super) fn collect_static_initializer_static_writes_from_call_args(
        &mut self,
        args: &[CallArg],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arg in args {
            match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => {
                    self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
                }
            }
        }
    }

    /// Recurse through statements reachable from an initializer expression and reject any write to static storage.
    pub(super) fn collect_static_initializer_static_writes_from_stmt(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::Assignment(_)
            | Statement::CompoundAssignment(_)
            | Statement::TupleUnpack(_)
            | Statement::ChainedAssignment(_) => {
                self.collect_static_initializer_binding_writes(stmt, current_static, visiting_functions);
            }
            Statement::FieldAssignment(_) | Statement::IndexAssignment(_) | Statement::TupleAssign(_) => {
                self.collect_static_initializer_place_writes(stmt, current_static, visiting_functions);
            }
            Statement::Assert(assert_stmt) => {
                self.collect_static_initializer_assert_writes(assert_stmt, current_static, visiting_functions);
            }
            Statement::If(if_stmt) => {
                self.collect_static_initializer_if_statement_writes(if_stmt, current_static, visiting_functions);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) | Statement::Break(Some(expr)) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Statement::Loop(loop_stmt) => {
                self.collect_static_initializer_body_writes(&loop_stmt.body, current_static, visiting_functions);
            }
            Statement::While(while_stmt) => {
                self.collect_static_initializer_static_writes_from_condition(
                    &while_stmt.condition,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_body_writes(&while_stmt.body, current_static, visiting_functions);
            }
            Statement::For(for_stmt) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &for_stmt.iter,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_body_writes(&for_stmt.body, current_static, visiting_functions);
            }
            Statement::Unsafe(unsafe_stmt) => {
                self.collect_static_initializer_body_writes(&unsafe_stmt.body, current_static, visiting_functions);
            }
            Statement::VocabExpressionItem(item) => {
                self.collect_static_initializer_static_writes_from_expr(&item.expr, current_static, visiting_functions);
                for modifier in &item.modifiers {
                    self.collect_static_initializer_static_writes_from_expr(
                        &modifier.value,
                        current_static,
                        visiting_functions,
                    );
                }
            }
            Statement::Return(None)
            | Statement::Pass
            | Statement::Break(None)
            | Statement::Continue
            | Statement::Surface(_)
            | Statement::VocabBlock(_) => {}
        }
    }

    /// Check direct binding assignments and named assignment lists selected by the statement coordinator.
    pub(super) fn collect_static_initializer_binding_writes(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::Assignment(assign) => {
                if self.static_decl_positions.contains_key(&assign.name) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        &assign.name,
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::CompoundAssignment(assign) => {
                if self.static_decl_positions.contains_key(&assign.name) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        &assign.name,
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::TupleUnpack(assign) => {
                self.collect_static_initializer_named_targets_writes(
                    &assign.names,
                    &assign.value,
                    stmt.span,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::ChainedAssignment(assign) => {
                self.collect_static_initializer_named_targets_writes(
                    &assign.targets,
                    &assign.value,
                    stmt.span,
                    current_static,
                    visiting_functions,
                );
            }
            _ => {}
        }
    }

    /// Check each named target before the common assignment value, retaining source diagnostic order.
    pub(super) fn collect_static_initializer_named_targets_writes(
        &mut self,
        targets: &[String],
        value: &Spanned<Expr>,
        span: Span,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for target in targets {
            if self.static_decl_positions.contains_key(target) {
                self.errors.push(errors::static_initializer_static_write_not_allowed(
                    current_static,
                    target,
                    span,
                ));
            }
        }
        self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
    }

    /// Check receiver and index expressions for the assignment-place forms selected by the statement coordinator.
    pub(super) fn collect_static_initializer_place_writes(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::FieldAssignment(assign) => {
                if let Some(target_static) = self.static_assignment_target_name(&assign.object) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        target_static.as_str(),
                        assign.target_span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.object,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::IndexAssignment(assign) => {
                if let Some(target_static) = self.static_assignment_target_name(&assign.object) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        target_static.as_str(),
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.object,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.index,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::TupleAssign(assign) => {
                for target in &assign.targets {
                    if let Some(target_static) = self.static_assignment_target_name(target) {
                        self.errors.push(errors::static_initializer_static_write_not_allowed(
                            current_static,
                            target_static.as_str(),
                            target.span,
                        ));
                    }
                    self.collect_static_initializer_static_writes_from_expr(target, current_static, visiting_functions);
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            _ => {}
        }
    }

    /// Check the assertion value before its optional message for every assertion form.
    pub(super) fn collect_static_initializer_assert_writes(
        &mut self,
        assert_stmt: &AssertStmt,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &assert_stmt.kind {
            AssertKind::Condition(condition) => {
                self.collect_static_initializer_static_writes_from_expr(condition, current_static, visiting_functions)
            }
            AssertKind::IsPattern { value, .. } => {
                self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions)
            }
            AssertKind::Raises { call, .. } => {
                self.collect_static_initializer_static_writes_from_expr(call, current_static, visiting_functions)
            }
        }
        if let Some(message) = &assert_stmt.message {
            self.collect_static_initializer_static_writes_from_expr(message, current_static, visiting_functions);
        }
    }

    /// Check an if statement's condition and branches in written order, including each elif condition.
    pub(super) fn collect_static_initializer_if_statement_writes(
        &mut self,
        if_stmt: &IfStmt,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_condition(
            &if_stmt.condition,
            current_static,
            visiting_functions,
        );
        for inner in &if_stmt.then_body {
            self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
        }
        for (condition, body) in &if_stmt.elif_branches {
            self.collect_static_initializer_static_writes_from_expr(condition, current_static, visiting_functions);
            for inner in body {
                self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
            }
        }
        if let Some(else_body) = &if_stmt.else_body {
            for inner in else_body {
                self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
            }
        }
    }

    /// Collect static writes performed inside initializer conditions.
    pub(super) fn collect_static_initializer_static_writes_from_condition(
        &mut self,
        condition: &Condition,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match condition {
            Condition::Expr(expr) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Condition::Let { value, .. } => {
                self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
            }
        }
    }

    /// Resolve the static root name for assignment targets such as `S`, `S.field`, or `S[idx]`.
    pub(super) fn static_assignment_target_name(&self, expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => self.static_decl_positions.contains_key(name).then(|| name.clone()),
            Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) => {
                self.static_assignment_target_name(object)
            }
            _ => None,
        }
    }
}
