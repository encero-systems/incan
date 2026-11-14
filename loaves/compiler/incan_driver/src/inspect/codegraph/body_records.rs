//! Import, declaration body, statement and expression records.

use super::*;

impl<'analysis> CodegraphBuilder<'analysis> {
    /// Add an import before containment, Rust references and public exports. Canonical binding evidence alone
    /// selects checked provenance; source/degraded imports do not gain authority by entering a helper.
    pub(super) fn collect_import_records(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        index: usize,
        import: &ImportDecl,
        span: Span,
        degraded: bool,
    ) {
        let import_id = import_id(module, index);
        let import_bindings = self.import_canonical_bindings(module, span);
        let provenance = if import_bindings
            .iter()
            .any(|binding| binding.canonical_identity.is_some())
        {
            CodegraphProvenance::Checked
        } else {
            CodegraphProvenance::Syntax
        };
        self.records.push(CodegraphRecord::Import(import_record(
            module,
            module_id,
            &import_id,
            import,
            span,
            import_bindings.clone(),
            provenance,
            degraded,
        )));
        self.records.push(CodegraphRecord::Containment(containment_record(
            module_id,
            &import_id,
            "module_contains_import",
            module,
            span,
            degraded,
        )));
        self.collect_rust_item_references(module, module_id, &import_id, import, span, &import_bindings, degraded);
        if import.visibility == Visibility::Public {
            for name in import_export_names(import) {
                let binding = import_bindings.iter().find(|binding| binding.local_name == name);
                let canonical_identity = binding.and_then(|binding| binding.canonical_identity.clone());
                let stable_identity = binding.and_then(|binding| binding.stable_identity.clone());
                self.records.push(CodegraphRecord::Export(export_record(
                    module,
                    module_id,
                    &import_id,
                    &name,
                    "import",
                    span,
                    canonical_identity,
                    stable_identity,
                    degraded,
                )));
            }
        }
    }

    /// Add body-level reference and call facts under a declaration owner.
    pub(super) fn collect_declaration_body_records(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: &str,
        declaration: &Spanned<Declaration>,
        degraded: bool,
    ) {
        match &declaration.node {
            Declaration::Const(decl) => self.collect_expr(module, module_id, Some(owner_id), &decl.value, degraded),
            Declaration::Static(decl) => self.collect_expr(module, module_id, Some(owner_id), &decl.value, degraded),
            Declaration::Model(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for field in &decl.fields {
                    if let Some(default) = &field.node.default {
                        self.collect_expr(module, module_id, Some(owner_id), default, degraded);
                    }
                }
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
                for property in &decl.properties {
                    if let Some(body) = &property.node.body {
                        self.collect_statements(module, module_id, Some(owner_id), body, degraded);
                    }
                }
            }
            Declaration::Class(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for field in &decl.fields {
                    if let Some(default) = &field.node.default {
                        self.collect_expr(module, module_id, Some(owner_id), default, degraded);
                    }
                }
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
                for property in &decl.properties {
                    if let Some(body) = &property.node.body {
                        self.collect_statements(module, module_id, Some(owner_id), body, degraded);
                    }
                }
            }
            Declaration::Trait(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
                for property in &decl.properties {
                    if let Some(body) = &property.node.body {
                        self.collect_statements(module, module_id, Some(owner_id), body, degraded);
                    }
                }
            }
            Declaration::Newtype(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for rebinding in &decl.rebindings {
                    self.collect_expr(module, module_id, Some(owner_id), &rebinding.node.target, degraded);
                }
                for edge in &decl.interop_edges {
                    self.collect_expr(module, module_id, Some(owner_id), &edge.node.adapter, degraded);
                }
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
            }
            Declaration::Enum(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                for method in &decl.methods {
                    self.collect_method_body_records(module, module_id, Some(owner_id), &method.node, degraded);
                }
            }
            Declaration::Function(decl) => {
                self.collect_decorators(module, module_id, Some(owner_id), &decl.decorators, degraded);
                self.collect_param_defaults(module, module_id, Some(owner_id), &decl.params, degraded);
                self.collect_statements(module, module_id, Some(owner_id), &decl.body, degraded);
            }
            Declaration::TestModule(decl) => {
                for nested in &decl.body {
                    self.collect_declaration_body_records(module, module_id, owner_id, nested, degraded);
                }
            }
            Declaration::Import(_)
            | Declaration::Alias(_)
            | Declaration::Partial(_)
            | Declaration::TypeAlias(_)
            | Declaration::VocabBlock(_)
            | Declaration::Capability(_)
            | Declaration::Docstring(_) => {}
        }
    }

    /// Collect method decorators, parameter defaults, and body facts under the enclosing declaration owner.
    pub(super) fn collect_method_body_records(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        method: &incan_frontend::ast::MethodDecl,
        degraded: bool,
    ) {
        self.collect_decorators(module, module_id, owner_id, &method.decorators, degraded);
        self.collect_param_defaults(module, module_id, owner_id, &method.params, degraded);
        if let Some(body) = &method.body {
            self.collect_statements(module, module_id, owner_id, body, degraded);
        }
    }

    /// Collect expression-valued decorator arguments.
    pub(super) fn collect_decorators(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        decorators: &[Spanned<Decorator>],
        degraded: bool,
    ) {
        for decorator in decorators {
            for arg in &decorator.node.args {
                match arg {
                    DecoratorArg::Positional(value) => {
                        self.collect_expr(module, module_id, owner_id, value, degraded);
                    }
                    DecoratorArg::Named(_, DecoratorArgValue::Expr(value)) => {
                        self.collect_expr(module, module_id, owner_id, value, degraded);
                    }
                    DecoratorArg::Named(_, DecoratorArgValue::Type(_)) => {}
                }
            }
        }
    }

    /// Collect default expressions attached to function or method parameters.
    pub(super) fn collect_param_defaults(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        params: &[Spanned<incan_frontend::ast::Param>],
        degraded: bool,
    ) {
        for param in params {
            if let Some(default) = &param.node.default {
                self.collect_expr(module, module_id, owner_id, default, degraded);
            }
        }
    }

    /// Collect expression facts from a statement list in source order.
    pub(super) fn collect_statements(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        statements: &[Spanned<Statement>],
        degraded: bool,
    ) {
        for statement in statements {
            self.collect_statement(module, module_id, owner_id, statement, degraded);
        }
    }

    /// Collect expression facts from one statement and its descendants.
    pub(super) fn collect_statement(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        statement: &Spanned<Statement>,
        degraded: bool,
    ) {
        match &statement.node {
            Statement::Assignment(stmt) => self.collect_expr(module, module_id, owner_id, &stmt.value, degraded),
            Statement::FieldAssignment(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.object, degraded);
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded);
            }
            Statement::IndexAssignment(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.object, degraded);
                self.collect_expr(module, module_id, owner_id, &stmt.index, degraded);
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) | Statement::Break(Some(expr)) => {
                self.collect_expr(module, module_id, owner_id, expr, degraded);
            }
            Statement::If(stmt) => {
                self.collect_condition(module, module_id, owner_id, &stmt.condition, degraded);
                self.collect_statements(module, module_id, owner_id, &stmt.then_body, degraded);
                for (condition, body) in &stmt.elif_branches {
                    self.collect_expr(module, module_id, owner_id, condition, degraded);
                    self.collect_statements(module, module_id, owner_id, body, degraded);
                }
                if let Some(body) = &stmt.else_body {
                    self.collect_statements(module, module_id, owner_id, body, degraded);
                }
            }
            Statement::Loop(stmt) => self.collect_statements(module, module_id, owner_id, &stmt.body, degraded),
            Statement::While(stmt) => {
                self.collect_condition(module, module_id, owner_id, &stmt.condition, degraded);
                self.collect_statements(module, module_id, owner_id, &stmt.body, degraded);
            }
            Statement::For(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.iter, degraded);
                self.collect_statements(module, module_id, owner_id, &stmt.body, degraded);
            }
            Statement::Unsafe(stmt) => self.collect_statements(module, module_id, owner_id, &stmt.body, degraded),
            Statement::VocabExpressionItem(item) => {
                self.collect_expr(module, module_id, owner_id, &item.expr, degraded);
                for modifier in &item.modifiers {
                    self.collect_expr(module, module_id, owner_id, &modifier.value, degraded);
                }
            }
            Statement::Assert(stmt) => {
                match &stmt.kind {
                    AssertKind::Condition(condition) => {
                        self.collect_expr(module, module_id, owner_id, condition, degraded);
                    }
                    AssertKind::IsPattern { value, .. } => {
                        self.collect_expr(module, module_id, owner_id, value, degraded);
                    }
                    AssertKind::Raises { call, .. } => {
                        self.collect_expr(module, module_id, owner_id, call, degraded);
                    }
                }
                if let Some(message) = &stmt.message {
                    self.collect_expr(module, module_id, owner_id, message, degraded);
                }
            }
            Statement::CompoundAssignment(stmt) => {
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded)
            }
            Statement::TupleUnpack(stmt) => self.collect_expr(module, module_id, owner_id, &stmt.value, degraded),
            Statement::TupleAssign(stmt) => {
                for target in &stmt.targets {
                    self.collect_expr(module, module_id, owner_id, target, degraded);
                }
                self.collect_expr(module, module_id, owner_id, &stmt.value, degraded);
            }
            Statement::ChainedAssignment(stmt) => self.collect_expr(module, module_id, owner_id, &stmt.value, degraded),
            Statement::Surface(stmt) => match &stmt.payload {
                SurfaceStmtPayload::KeywordArgs(args) => {
                    for arg in args {
                        self.collect_expr(module, module_id, owner_id, arg, degraded);
                    }
                }
            },
            Statement::VocabBlock(block) => {
                self.collect_decorators(module, module_id, owner_id, &block.decorators, degraded);
                for arg in &block.header_args {
                    self.collect_expr(module, module_id, owner_id, arg, degraded);
                }
                self.collect_statements(module, module_id, owner_id, &block.body, degraded);
            }
            Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => {}
        }
    }

    /// Collect expression facts from a condition.
    pub(super) fn collect_condition(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        condition: &Condition,
        degraded: bool,
    ) {
        match condition {
            Condition::Expr(expr) | Condition::Let { value: expr, .. } => {
                self.collect_expr(module, module_id, owner_id, expr, degraded);
            }
        }
    }

    /// Collect expression facts from one call argument.
    pub(super) fn collect_call_arg(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        arg: &CallArg,
        degraded: bool,
    ) {
        match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => self.collect_expr(module, module_id, owner_id, expr, degraded),
        }
    }

    /// Collect source-level reference and call facts from one expression.
    pub(super) fn collect_expr(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        expr: &Spanned<Expr>,
        degraded: bool,
    ) {
        let context = ExpressionRecordContext {
            module,
            module_id,
            owner_id,
            degraded,
        };
        match &expr.node {
            Expr::Ident(name) => {
                self.push_reference(module, module_id, owner_id, name, "identifier", expr.span, degraded)
            }
            Expr::SelfExpr => self.push_reference(module, module_id, owner_id, "self", "self", expr.span, degraded),
            Expr::Literal(_) => {}
            Expr::Binary(left, _, right) | Expr::Index(left, right) => {
                self.collect_expr(module, module_id, owner_id, left, degraded);
                self.collect_expr(module, module_id, owner_id, right, degraded);
            }
            Expr::Unary(_, value) | Expr::Try(value) | Expr::Paren(value) => {
                self.collect_expr(module, module_id, owner_id, value, degraded);
            }
            Expr::Call(..) | Expr::MethodCall(..) | Expr::Constructor(..) => {
                self.collect_callable_expression(context, expr)
            }
            Expr::Partial(partial) => {
                self.collect_expr(module, module_id, owner_id, &partial.target, degraded);
                for arg in &partial.args {
                    self.collect_expr(module, module_id, owner_id, &arg.value, degraded);
                }
            }
            Expr::Slice(base, slice) => {
                self.collect_expr(module, module_id, owner_id, base, degraded);
                for value in [&slice.start, &slice.end, &slice.step].into_iter().flatten() {
                    self.collect_expr(module, module_id, owner_id, value, degraded);
                }
            }
            Expr::Field(base, field) => {
                self.collect_expr(module, module_id, owner_id, base, degraded);
                self.push_reference(module, module_id, owner_id, field, "field", expr.span, degraded);
            }
            Expr::Match(scrutinee, arms) => self.collect_match_expression(context, scrutinee, arms),
            Expr::If(if_expr) => {
                self.collect_expr(module, module_id, owner_id, &if_expr.condition, degraded);
                self.collect_statements(module, module_id, owner_id, &if_expr.then_body, degraded);
                if let Some(body) = &if_expr.else_body {
                    self.collect_statements(module, module_id, owner_id, body, degraded);
                }
            }
            Expr::Loop(loop_expr) => self.collect_statements(module, module_id, owner_id, &loop_expr.body, degraded),
            Expr::ListComp(comp) => {
                self.collect_expr(module, module_id, owner_id, &comp.expr, degraded);
                self.collect_comprehension_clauses(module, module_id, owner_id, &comp.clauses, degraded);
            }
            Expr::DictComp(comp) => {
                self.collect_expr(module, module_id, owner_id, &comp.key, degraded);
                self.collect_expr(module, module_id, owner_id, &comp.value, degraded);
                self.collect_comprehension_clauses(module, module_id, owner_id, &comp.clauses, degraded);
            }
            Expr::Generator(generator) => {
                self.collect_expr(module, module_id, owner_id, &generator.expr, degraded);
                self.collect_comprehension_clauses(module, module_id, owner_id, &generator.clauses, degraded);
            }
            Expr::Closure(params, body) => {
                self.collect_param_defaults(module, module_id, owner_id, params, degraded);
                self.collect_expr(module, module_id, owner_id, body, degraded);
            }
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_expr(module, module_id, owner_id, item, degraded);
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.collect_expr(module, module_id, owner_id, value, degraded);
                        }
                    }
                }
            }
            Expr::Dict(entries) => self.collect_dictionary_entries(context, entries),
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.collect_expr(module, module_id, owner_id, expr, degraded);
                    }
                }
            }
            Expr::Yield(Some(value)) => self.collect_expr(module, module_id, owner_id, value, degraded),
            Expr::Range { start, end, .. } => {
                self.collect_expr(module, module_id, owner_id, start, degraded);
                self.collect_expr(module, module_id, owner_id, end, degraded);
            }
            Expr::Surface(surface) => self.collect_surface_expression(context, &surface.payload, expr.span),
            Expr::VocabBlock(block) => {
                self.collect_decorators(module, module_id, owner_id, &block.decorators, degraded);
                for arg in &block.header_args {
                    self.collect_expr(module, module_id, owner_id, arg, degraded);
                }
                self.collect_statements(module, module_id, owner_id, &block.body, degraded);
            }
            // A fragment's DSL-owned structural content (tags, selectors, declarations, ...) has no ordinary Incan
            // symbol references of its own — only its expression holes are genuine Incan, so only they contribute
            // codegraph references and calls. `holes` is the single authority on which those are (RFC 081, #1022).
            Expr::Embedded(fragment) => {
                for hole in fragment.holes() {
                    self.collect_expr(module, module_id, owner_id, hole, degraded);
                }
            }
            Expr::Yield(None) => {}
        }
    }

    /// Record callable forms before walking their receiver/callee and arguments. The dispatcher admits only Call,
    /// MethodCall and Constructor here; other expression variants remain owned by its ordinary walk.
    pub(super) fn collect_callable_expression(&mut self, context: ExpressionRecordContext<'_>, expr: &Spanned<Expr>) {
        let ExpressionRecordContext {
            module,
            module_id,
            owner_id,
            degraded,
        } = context;
        match &expr.node {
            Expr::Call(callee, type_args, args) => {
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    &expr_label(&callee.node),
                    "function",
                    args.len(),
                    type_args.len(),
                    expr.span,
                    codegraph_callee_identity_span(callee),
                    degraded,
                );
                self.collect_expr(module, module_id, owner_id, callee, degraded);
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
            Expr::MethodCall(receiver, method, type_args, args) => {
                // Imported module and declared type calls retain their qualified label rather than collapsing
                // onto an unrelated same-name local method.
                let qualified;
                let callee_label = if Self::receiver_names_an_imported_module(module, receiver)
                    || Self::receiver_names_a_declared_type(module, receiver)
                {
                    qualified = format!("{}.{method}", expr_label(&receiver.node));
                    qualified.as_str()
                } else {
                    method.as_str()
                };
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    callee_label,
                    "method",
                    args.len(),
                    type_args.len(),
                    expr.span,
                    expr.span,
                    degraded,
                );
                self.collect_expr(module, module_id, owner_id, receiver, degraded);
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
            Expr::Constructor(name, args) => {
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    name,
                    "constructor",
                    args.len(),
                    0,
                    expr.span,
                    expr.span,
                    degraded,
                );
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
            _ => {}
        }
    }

    /// Walk the match scrutinee before each arm's guard and body in source order. Patterns remain outside this
    /// expression walk, and block arms retain the ordinary statement collector rather than inventing references.
    pub(super) fn collect_match_expression(
        &mut self,
        context: ExpressionRecordContext<'_>,
        scrutinee: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
    ) {
        let ExpressionRecordContext {
            module,
            module_id,
            owner_id,
            degraded,
        } = context;
        self.collect_expr(module, module_id, owner_id, scrutinee, degraded);
        for arm in arms {
            if let Some(guard) = &arm.node.guard {
                self.collect_expr(module, module_id, owner_id, guard, degraded);
            }
            match &arm.node.body {
                MatchBody::Expr(value) => self.collect_expr(module, module_id, owner_id, value, degraded),
                MatchBody::Block(body) => self.collect_statements(module, module_id, owner_id, body, degraded),
            }
        }
    }

    /// Walk dictionary key/value pairs and spreads in source order, using the same owner and provenance context as
    /// the surrounding expression. Literal keys retain the ordinary expression walk's no-reference behavior.
    pub(super) fn collect_dictionary_entries(&mut self, context: ExpressionRecordContext<'_>, entries: &[DictEntry]) {
        let ExpressionRecordContext {
            module,
            module_id,
            owner_id,
            degraded,
        } = context;
        for entry in entries {
            match entry {
                DictEntry::Pair(key, value) => {
                    self.collect_expr(module, module_id, owner_id, key, degraded);
                    self.collect_expr(module, module_id, owner_id, value, degraded);
                }
                DictEntry::Spread(value) => self.collect_expr(module, module_id, owner_id, value, degraded),
            }
        }
    }

    /// Walk surface payloads without reconstructing their DSL semantics. Prefix/race/glyph operands retain their
    /// expression/body order; explicit paths and symbols keep the authored surface span and record kind.
    pub(super) fn collect_surface_expression(
        &mut self,
        context: ExpressionRecordContext<'_>,
        payload: &SurfaceExprPayload,
        span: Span,
    ) {
        let ExpressionRecordContext {
            module,
            module_id,
            owner_id,
            degraded,
        } = context;
        match payload {
            SurfaceExprPayload::PrefixUnary(value) => self.collect_expr(module, module_id, owner_id, value, degraded),
            SurfaceExprPayload::RaceFor(race) => {
                for arm in &race.arms {
                    self.collect_expr(module, module_id, owner_id, &arm.awaitable, degraded);
                    match &arm.body {
                        RaceForBody::Expr(value) => self.collect_expr(module, module_id, owner_id, value, degraded),
                        RaceForBody::Block(body) => {
                            self.collect_statements(module, module_id, owner_id, body, degraded)
                        }
                    }
                }
            }
            SurfaceExprPayload::LeadingDotPath { segments, .. } => {
                self.push_reference(
                    module,
                    module_id,
                    owner_id,
                    &segments.join("."),
                    "surface_path",
                    span,
                    degraded,
                );
            }
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.collect_expr(module, module_id, owner_id, left, degraded);
                self.collect_expr(module, module_id, owner_id, right, degraded);
            }
            SurfaceExprPayload::ScopedSymbolCall { symbol, args, .. } => {
                self.push_call(
                    module,
                    module_id,
                    owner_id,
                    symbol,
                    "surface_symbol",
                    args.len(),
                    0,
                    span,
                    span,
                    degraded,
                );
                for arg in args {
                    self.collect_call_arg(module, module_id, owner_id, arg, degraded);
                }
            }
        }
    }

    /// Collect comprehension clause expressions.
    pub(super) fn collect_comprehension_clauses(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        clauses: &[ComprehensionClause],
        degraded: bool,
    ) {
        for clause in clauses {
            match clause {
                ComprehensionClause::For { iter, .. } | ComprehensionClause::If(iter) => {
                    self.collect_expr(module, module_id, owner_id, iter, degraded);
                }
            }
        }
    }
}
