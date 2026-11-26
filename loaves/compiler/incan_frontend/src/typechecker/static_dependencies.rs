//! Static initializer dependency collection and cycle validation.

use super::*;

impl TypeChecker {
    /// Validate local static dependencies before declaration checking.
    ///
    /// `static` initializers may only reference earlier local statics, and dependency cycles are rejected.
    pub(super) fn validate_static_dependencies(&mut self) {
        if self.static_decls.is_empty() {
            return;
        }

        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        let static_spans: HashMap<String, Span> = self
            .static_decls
            .iter()
            .map(|(static_decl, span)| (static_decl.name.clone(), *span))
            .collect();

        for (static_decl, _) in self.static_decls.clone() {
            let mut deps = HashSet::new();
            let mut visiting_functions = HashSet::new();
            self.collect_static_dependencies_from_expr(&static_decl.value.node, &mut deps, &mut visiting_functions);
            self.collect_static_initializer_static_writes_from_expr(
                &static_decl.value,
                &static_decl.name,
                &mut HashSet::new(),
            );

            if let Some(current_idx) = self.static_decl_positions.get(&static_decl.name).copied() {
                for dep in &deps {
                    if let Some(dep_idx) = self.static_decl_positions.get(dep).copied()
                        && dep_idx >= current_idx
                    {
                        self.errors.push(errors::static_initializer_requires_earlier_static(
                            dep,
                            &static_decl.name,
                            static_decl.value.span,
                        ));
                    }
                }
            }

            graph.insert(static_decl.name.clone(), deps.into_iter().collect());
        }

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum VisitState {
            Visiting,
            Done,
        }

        fn visit(
            name: &str,
            graph: &HashMap<String, Vec<String>>,
            spans: &HashMap<String, Span>,
            state: &mut HashMap<String, VisitState>,
            stack: &mut Vec<String>,
            collected_errors: &mut Vec<CompileError>,
        ) {
            match state.get(name).copied() {
                Some(VisitState::Done) => return,
                Some(VisitState::Visiting) => {
                    if let Some(start_idx) = stack.iter().position(|item| item == name) {
                        let mut cycle = stack[start_idx..].to_vec();
                        cycle.push(name.to_string());
                        collected_errors.push(errors::static_dependency_cycle(
                            &cycle.join(" -> "),
                            spans.get(name).copied().unwrap_or_default(),
                        ));
                    }
                    return;
                }
                None => {}
            }

            state.insert(name.to_string(), VisitState::Visiting);
            stack.push(name.to_string());
            if let Some(deps) = graph.get(name) {
                for dep in deps {
                    visit(dep, graph, spans, state, stack, collected_errors);
                }
            }
            stack.pop();
            state.insert(name.to_string(), VisitState::Done);
        }

        let mut state: HashMap<String, VisitState> = HashMap::new();
        let mut stack = Vec::new();
        for name in graph.keys() {
            visit(name, &graph, &static_spans, &mut state, &mut stack, &mut self.errors);
        }
    }

    /// Recursively collect the names of local static dependencies from `expr` into `deps`.
    pub(super) fn collect_static_dependencies_from_expr(
        &self,
        expr: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name) => {
                if self.static_decl_positions.contains_key(name) {
                    deps.insert(name.clone());
                }
            }
            Expr::Literal(_) | Expr::SelfExpr => {}
            Expr::Binary(left, _, right) | Expr::Index(left, right) => {
                self.collect_static_dependencies_from_expr(&left.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&right.node, deps, visiting_functions);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
                self.collect_static_dependencies_from_expr(&inner.node, deps, visiting_functions);
            }
            Expr::Yield(None) => {}
            Expr::Call(func, _type_args, args) => {
                self.collect_static_dependencies_from_expr(&func.node, deps, visiting_functions);
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
                self.collect_local_function_static_dependencies(&func.node, deps, visiting_functions);
            }
            Expr::Slice(target, slice) => {
                self.collect_static_dependencies_from_expr(&target.node, deps, visiting_functions);
                if let Some(start) = &slice.start {
                    self.collect_static_dependencies_from_expr(&start.node, deps, visiting_functions);
                }
                if let Some(end) = &slice.end {
                    self.collect_static_dependencies_from_expr(&end.node, deps, visiting_functions);
                }
                if let Some(step) = &slice.step {
                    self.collect_static_dependencies_from_expr(&step.node, deps, visiting_functions);
                }
            }
            Expr::Field(object, _) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
            }
            Expr::MethodCall(object, _, _type_args, args) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
            Expr::Partial(partial) => {
                self.collect_static_dependencies_from_expr(&partial.target.node, deps, visiting_functions);
                for arg in &partial.args {
                    self.collect_static_dependencies_from_expr(&arg.value.node, deps, visiting_functions);
                }
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_static_dependencies_from_expr(&scrutinee.node, deps, visiting_functions);
                self.collect_match_static_dependencies(arms, deps, visiting_functions);
            }
            Expr::If(if_expr) => {
                self.collect_if_static_dependencies(if_expr, deps, visiting_functions);
            }
            Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Expr::Generator(_) | Expr::ListComp(_) | Expr::DictComp(_) => {
                self.collect_comprehension_static_dependencies(expr, deps, visiting_functions);
            }
            Expr::Closure(_, _) => {}
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_static_dependencies_from_expr(&item.node, deps, visiting_functions);
                }
            }
            Expr::List(items) => {
                for item in items {
                    match item {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                        }
                    }
                }
            }
            Expr::Dict(items) => {
                self.collect_dict_static_dependencies(items, deps, visiting_functions);
            }
            Expr::Constructor(_, args) => {
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                    }
                }
            }
            Expr::Range { start, end, .. } => {
                self.collect_static_dependencies_from_expr(&start.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&end.node, deps, visiting_functions);
            }
            Expr::Surface(surface) => {
                self.collect_surface_static_dependencies(&surface.payload, deps, visiting_functions)
            }
            Expr::VocabBlock(block) => {
                for arg in &block.header_args {
                    self.collect_static_dependencies_from_expr(&arg.node, deps, visiting_functions);
                }
                for stmt in &block.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            // Only expression holes belong to ordinary Incan; the surrounding fragment is DSL-owned (RFC 081).
            Expr::Embedded(fragment) => {
                for hole in fragment.holes() {
                    self.collect_static_dependencies_from_expr(&hole.node, deps, visiting_functions);
                }
            }
        }
    }

    /// Collect match-arm dependencies in guard-before-body order after the scrutinee has been visited.
    pub(super) fn collect_match_static_dependencies(
        &self,
        arms: &[Spanned<MatchArm>],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arm in arms {
            if let Some(guard) = &arm.node.guard {
                self.collect_static_dependencies_from_expr(&guard.node, deps, visiting_functions);
            }
            match &arm.node.body {
                MatchBody::Expr(expr) => {
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions)
                }
                MatchBody::Block(stmts) => {
                    for stmt in stmts {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
            }
        }
    }

    /// Follow a directly called local helper once on the active stack, leaving closure creation unevaluated.
    pub(super) fn collect_local_function_static_dependencies(
        &self,
        callee: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        if let Expr::Ident(name) = callee
            && let Some(declaration) = self.local_function_decls.get(name)
            && visiting_functions.insert(name.clone())
        {
            for stmt in &declaration.body {
                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
            }
            visiting_functions.remove(name);
        }
    }

    /// Collect an if expression's condition and both branch bodies in the original traversal order.
    pub(super) fn collect_if_static_dependencies(
        &self,
        expr: &IfExpr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_dependencies_from_expr(&expr.condition.node, deps, visiting_functions);
        for stmt in &expr.then_body {
            self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
        }
        if let Some(body) = &expr.else_body {
            for stmt in body {
                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
            }
        }
    }

    /// Collect result, source and filter dependencies for the comprehension forms selected by the expression visitor.
    pub(super) fn collect_comprehension_static_dependencies(
        &self,
        expr: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Generator(generator) => {
                self.collect_static_dependencies_from_expr(&generator.expr.node, deps, visiting_functions);
                for clause in &generator.clauses {
                    let expr = match clause {
                        ComprehensionClause::For { iter, .. } => iter,
                        ComprehensionClause::If(condition) => condition,
                    };
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                }
            }
            Expr::ListComp(comp) => {
                self.collect_static_dependencies_from_expr(&comp.expr.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&comp.iter.node, deps, visiting_functions);
                if let Some(filter) = &comp.filter {
                    self.collect_static_dependencies_from_expr(&filter.node, deps, visiting_functions);
                }
            }
            Expr::DictComp(comp) => {
                self.collect_static_dependencies_from_expr(&comp.key.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&comp.value.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&comp.iter.node, deps, visiting_functions);
                if let Some(filter) = &comp.filter {
                    self.collect_static_dependencies_from_expr(&filter.node, deps, visiting_functions);
                }
            }
            _ => {}
        }
    }

    /// Collect dictionary keys, values and spread dependencies in their written order.
    pub(super) fn collect_dict_static_dependencies(
        &self,
        items: &[DictEntry],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for item in items {
            match item {
                DictEntry::Pair(key, value) => {
                    self.collect_static_dependencies_from_expr(&key.node, deps, visiting_functions);
                    self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                }
                DictEntry::Spread(value) => {
                    self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions)
                }
            }
        }
    }

    /// Collect ordinary Incan dependencies within surface payloads without visiting DSL-owned syntax.
    pub(super) fn collect_surface_static_dependencies(
        &self,
        payload: &SurfaceExprPayload,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match payload {
            SurfaceExprPayload::PrefixUnary(expr) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            SurfaceExprPayload::RaceFor(race) => {
                for arm in &race.arms {
                    self.collect_static_dependencies_from_expr(&arm.awaitable.node, deps, visiting_functions);
                    match &arm.body {
                        RaceForBody::Expr(expr) => {
                            self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                        }
                        RaceForBody::Block(stmts) => {
                            for stmt in stmts {
                                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                            }
                        }
                    }
                }
            }
            SurfaceExprPayload::LeadingDotPath { .. } => {}
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.collect_static_dependencies_from_expr(&left.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&right.node, deps, visiting_functions);
            }
            SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
        }
    }

    /// Recursively collect the names of local static dependencies from `args` into `deps`.
    pub(super) fn collect_static_dependencies_from_call_args(
        &self,
        args: &[CallArg],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arg in args {
            match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => {
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                }
            }
        }
    }

    /// Recursively collect the names of local static dependencies from `stmt` into `deps`.
    pub(super) fn collect_static_dependencies_from_statement(
        &self,
        stmt: &Statement,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Assignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::FieldAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::IndexAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.index.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            Statement::Break(Some(expr)) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => {}
            Statement::If(if_stmt) => {
                self.collect_static_dependencies_from_condition(&if_stmt.condition, deps, visiting_functions);
                for stmt in &if_stmt.then_body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
                for (condition, body) in &if_stmt.elif_branches {
                    self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                    for stmt in body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
                if let Some(else_body) = &if_stmt.else_body {
                    for stmt in else_body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
            }
            Statement::Loop(loop_stmt) => {
                for stmt in &loop_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::While(while_stmt) => {
                self.collect_static_dependencies_from_condition(&while_stmt.condition, deps, visiting_functions);
                for stmt in &while_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::For(for_stmt) => {
                self.collect_static_dependencies_from_expr(&for_stmt.iter.node, deps, visiting_functions);
                for stmt in &for_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::Unsafe(unsafe_stmt) => {
                for stmt in &unsafe_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::CompoundAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::TupleUnpack(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::TupleAssign(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
                for target in &assign.targets {
                    self.collect_static_dependencies_from_expr(&target.node, deps, visiting_functions);
                }
            }
            Statement::ChainedAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::VocabExpressionItem(item) => {
                self.collect_static_dependencies_from_expr(&item.expr.node, deps, visiting_functions);
                for modifier in &item.modifiers {
                    self.collect_static_dependencies_from_expr(&modifier.value.node, deps, visiting_functions);
                }
            }
            Statement::Assert(assert_stmt) => {
                match &assert_stmt.kind {
                    AssertKind::Condition(condition) => {
                        self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                    }
                    AssertKind::IsPattern { value, .. } => {
                        self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                    }
                    AssertKind::Raises { call, .. } => {
                        self.collect_static_dependencies_from_expr(&call.node, deps, visiting_functions);
                    }
                }
                if let Some(message) = &assert_stmt.message {
                    self.collect_static_dependencies_from_expr(&message.node, deps, visiting_functions);
                }
            }
            Statement::Surface(_) | Statement::VocabBlock(_) => {}
        }
    }

    /// Collect static dependencies referenced by a condition expression.
    pub(super) fn collect_static_dependencies_from_condition(
        &self,
        condition: &Condition,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match condition {
            Condition::Expr(expr) => self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions),
            Condition::Let { value, .. } => {
                self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
            }
        }
    }
}
