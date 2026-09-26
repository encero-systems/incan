//! Refusal of source names that use the compiler's reserved `__incan_` prefix (#1769).
//!
//! The compiler spells the items and locals it generates with `__incan_` (the original a decorator wraps, lowering
//! temporaries, projected symbol names), so a source name with that prefix could collide with a generated one and stop
//! the build on a duplicate definition. This pass walks the program's own declarations and bodies before collection and
//! refuses every name the source declares or binds with the prefix: module declarations and their members, parameters
//! and type parameters, local and pattern bindings, closure parameters and import aliases. Uses of a name are not
//! reported; the declaration is.
//!
//! Two kinds of name stay allowed. A type's `__incan_new` method is the constructor hook the compiler looks up by that
//! exact name. A node the compiler synthesized, such as the hidden helper import a vocabulary desugarer receives,
//! carries no source span (see [`crate::vocab_ast_bridge::is_synthetic_span`]) and is the compiler's own name.

use std::collections::HashSet;

use super::TypeChecker;
use crate::ast::{
    AssertKind, BindingKind, CallArg, ComprehensionClause, Condition, Declaration, DictEntry, EmbeddedFragmentExpr,
    Expr, FStringPart, ImportDecl, ImportKind, ListEntry, MatchBody, MethodDecl, Param, Pattern, PatternArg, Program,
    RaceForBody, Span, Spanned, Statement, SurfaceExprPayload, SurfaceStmtPayload, TypeParam,
};
use crate::diagnostics::{CompileError, errors};
use crate::vocab_ast_bridge::is_synthetic_span;
use incan_lang::lang::conventions::{RESERVED_COMPILER_NAME_PREFIX, TYPE_CONSTRUCTOR_HOOK};

/// Collects the reserved-prefix names one program declares or binds, one report per source position.
struct ReservedNameWalk {
    /// Refusals in source order.
    refusals: Vec<CompileError>,
    /// Names and positions already reported, so a node reached twice (a comprehension's mirrored first clause)
    /// reports once while two names of one import declaration each report.
    reported: HashSet<(String, usize, usize)>,
}

impl ReservedNameWalk {
    /// Refuse `name` as a `kind` declared at `span` when it uses the reserved prefix and the source spelled it.
    fn check(&mut self, name: &str, kind: &str, span: Span) {
        if !name.starts_with(RESERVED_COMPILER_NAME_PREFIX) || is_synthetic_span(span) {
            return;
        }
        if self.reported.insert((name.to_string(), span.start, span.end)) {
            self.refusals.push(errors::reserved_compiler_name(name, kind, span));
        }
    }

    // ---- Declarations ----

    /// Walk the names one module-level declaration introduces, then its members and bodies.
    fn declaration(&mut self, declaration: &Spanned<Declaration>) {
        let span = declaration.span;
        match &declaration.node {
            Declaration::Import(import) => self.import(import, span),
            Declaration::Const(constant) => {
                self.check(&constant.name, "constant", span);
                self.expr(&constant.value);
            }
            Declaration::Static(static_decl) => {
                self.check(&static_decl.name, "static", span);
                self.expr(&static_decl.value);
            }
            Declaration::Model(model) => {
                self.check(&model.name, "model", span);
                self.type_params(&model.type_params);
                for field in &model.fields {
                    self.check(&field.node.name, "field", field.span);
                    if let Some(default) = &field.node.default {
                        self.expr(default);
                    }
                }
                for alias in &model.method_aliases {
                    self.check(&alias.node.name, "method alias", alias.span);
                }
                for partial in &model.method_partials {
                    self.check(&partial.node.name, "method partial", partial.span);
                }
                for property in &model.properties {
                    self.check(&property.node.name, "property", property.span);
                    if let Some(body) = &property.node.body {
                        self.block(body);
                    }
                }
                self.methods(&model.methods);
            }
            Declaration::Class(class) => {
                self.check(&class.name, "class", span);
                self.type_params(&class.type_params);
                for field in &class.fields {
                    self.check(&field.node.name, "field", field.span);
                    if let Some(default) = &field.node.default {
                        self.expr(default);
                    }
                }
                for alias in &class.method_aliases {
                    self.check(&alias.node.name, "method alias", alias.span);
                }
                for partial in &class.method_partials {
                    self.check(&partial.node.name, "method partial", partial.span);
                }
                for property in &class.properties {
                    self.check(&property.node.name, "property", property.span);
                    if let Some(body) = &property.node.body {
                        self.block(body);
                    }
                }
                self.methods(&class.methods);
            }
            Declaration::Trait(trait_decl) => {
                self.check(&trait_decl.name, "trait", span);
                self.type_params(&trait_decl.type_params);
                for alias in &trait_decl.method_aliases {
                    self.check(&alias.node.name, "method alias", alias.span);
                }
                for partial in &trait_decl.method_partials {
                    self.check(&partial.node.name, "method partial", partial.span);
                }
                for property in &trait_decl.properties {
                    self.check(&property.node.name, "property", property.span);
                    if let Some(body) = &property.node.body {
                        self.block(body);
                    }
                }
                self.methods(&trait_decl.methods);
            }
            Declaration::Capability(capability) => self.check(&capability.name, "capability", span),
            Declaration::Alias(alias) => self.check(&alias.name, "alias", span),
            Declaration::Partial(partial) => self.check(&partial.name, "partial", span),
            Declaration::TypeAlias(alias) => {
                self.check(&alias.name, "type alias", span);
                self.type_params(&alias.type_params);
            }
            Declaration::Newtype(newtype) => {
                self.check(&newtype.name, "type", span);
                self.type_params(&newtype.type_params);
                for alias in &newtype.method_aliases {
                    self.check(&alias.node.name, "method alias", alias.span);
                }
                for partial in &newtype.method_partials {
                    self.check(&partial.node.name, "method partial", partial.span);
                }
                self.methods(&newtype.methods);
            }
            Declaration::Enum(enum_decl) => {
                self.check(&enum_decl.name, "enum", span);
                self.type_params(&enum_decl.type_params);
                for variant in &enum_decl.variants {
                    self.check(&variant.node.name, "variant", variant.span);
                }
                for alias in &enum_decl.variant_aliases {
                    self.check(&alias.node.name, "variant alias", alias.span);
                }
                self.methods(&enum_decl.methods);
            }
            Declaration::Function(function) => {
                self.check(&function.name, "function", span);
                self.type_params(&function.type_params);
                self.params(&function.params);
                self.block(&function.body);
            }
            Declaration::TestModule(module) => {
                for nested in &module.body {
                    self.declaration(nested);
                }
            }
            // A vocabulary block is desugared before the checker runs, and a docstring binds nothing.
            Declaration::VocabBlock(_) | Declaration::Docstring(_) => {}
        }
    }

    /// Walk the name an import binds in this module: the alias when the source writes one, otherwise the imported
    /// module's or item's own last segment.
    fn import(&mut self, import: &ImportDecl, span: Span) {
        let items = match &import.kind {
            ImportKind::From { items, .. } | ImportKind::PubFrom { items, .. } | ImportKind::RustFrom { items, .. } => {
                items
            }
            ImportKind::Module(path) => {
                self.import_binding(import.alias.as_deref(), path.segments.last().map(String::as_str), span);
                return;
            }
            ImportKind::PubLibrary { library, path } => {
                self.import_binding(
                    import.alias.as_deref(),
                    path.last().or(Some(library)).map(String::as_str),
                    span,
                );
                return;
            }
            ImportKind::RustCrate { crate_name, path, .. } => {
                self.import_binding(
                    import.alias.as_deref(),
                    path.last().or(Some(crate_name)).map(String::as_str),
                    span,
                );
                return;
            }
            ImportKind::Python(_) => {
                self.import_binding(import.alias.as_deref(), None, span);
                return;
            }
        };
        for item in items {
            self.import_binding(item.alias.as_deref(), Some(item.name.as_str()), span);
        }
    }

    /// Check the one name an import binds: its `alias` when the source writes one, otherwise the `imported` name.
    fn import_binding(&mut self, alias: Option<&str>, imported: Option<&str>, span: Span) {
        match (alias, imported) {
            (Some(alias), _) => self.check(alias, "import alias", span),
            (None, Some(imported)) => self.check(imported, "imported name", span),
            (None, None) => {}
        }
    }

    /// Walk the methods of a type, exempting the constructor hook the compiler looks up by name.
    fn methods(&mut self, methods: &[Spanned<MethodDecl>]) {
        for method in methods {
            if method.node.name != TYPE_CONSTRUCTOR_HOOK {
                self.check(&method.node.name, "method", method.span);
            }
            self.type_params(&method.node.type_params);
            self.params(&method.node.params);
            if let Some(body) = &method.node.body {
                self.block(body);
            }
        }
    }

    /// Walk declared type parameters.
    fn type_params(&mut self, type_params: &[TypeParam]) {
        for type_param in type_params {
            self.check(&type_param.name, "type parameter", type_param.span);
        }
    }

    /// Walk callable parameters and their default values.
    fn params(&mut self, params: &[Spanned<Param>]) {
        for param in params {
            self.check(&param.node.name, "parameter", param.span);
            if let Some(default) = &param.node.default {
                self.expr(default);
            }
        }
    }

    // ---- Statements ----

    /// Walk every statement of a block.
    fn block(&mut self, statements: &[Spanned<Statement>]) {
        for statement in statements {
            self.statement(statement);
        }
    }

    /// Walk the bindings one statement introduces, then its nested expressions and blocks.
    ///
    /// A reassignment and a compound assignment write an existing binding, so only a first binding is a
    /// declaration; the binding the name already has was reported where it was declared.
    fn statement(&mut self, statement: &Spanned<Statement>) {
        match &statement.node {
            Statement::Assignment(assignment) => {
                if assignment.binding != BindingKind::Reassign {
                    self.check(&assignment.name, "binding", assignment.name_span);
                }
                self.expr(&assignment.value);
            }
            Statement::TupleUnpack(unpack) => {
                if unpack.binding != BindingKind::Reassign {
                    for (name, span) in unpack.names.iter().zip(&unpack.name_spans) {
                        self.check(name, "binding", *span);
                    }
                }
                self.expr(&unpack.value);
            }
            Statement::ChainedAssignment(chained) => {
                if chained.binding != BindingKind::Reassign {
                    for (name, span) in chained.targets.iter().zip(&chained.target_spans) {
                        self.check(name, "binding", *span);
                    }
                }
                self.expr(&chained.value);
            }
            Statement::For(for_stmt) => {
                self.pattern(&for_stmt.pattern);
                self.expr(&for_stmt.iter);
                self.block(&for_stmt.body);
            }
            Statement::If(if_stmt) => {
                self.condition(&if_stmt.condition);
                self.block(&if_stmt.then_body);
                for (condition, body) in &if_stmt.elif_branches {
                    self.expr(condition);
                    self.block(body);
                }
                if let Some(else_body) = &if_stmt.else_body {
                    self.block(else_body);
                }
            }
            Statement::While(while_stmt) => {
                self.condition(&while_stmt.condition);
                self.block(&while_stmt.body);
            }
            Statement::Loop(loop_stmt) => self.block(&loop_stmt.body),
            Statement::Unsafe(unsafe_stmt) => self.block(&unsafe_stmt.body),
            Statement::Assert(assert) => {
                match &assert.kind {
                    AssertKind::Condition(condition) => self.expr(condition),
                    AssertKind::IsPattern { value, pattern } => {
                        self.expr(value);
                        self.pattern(pattern);
                    }
                    AssertKind::Raises { call, .. } => self.expr(call),
                }
                if let Some(message) = &assert.message {
                    self.expr(message);
                }
            }
            Statement::FieldAssignment(assignment) => {
                self.expr(&assignment.object);
                self.expr(&assignment.value);
            }
            Statement::IndexAssignment(assignment) => {
                self.expr(&assignment.object);
                self.expr(&assignment.index);
                self.expr(&assignment.value);
            }
            Statement::CompoundAssignment(assignment) => self.expr(&assignment.value),
            Statement::TupleAssign(assignment) => {
                for target in &assignment.targets {
                    self.expr(target);
                }
                self.expr(&assignment.value);
            }
            Statement::Return(value) | Statement::Break(value) => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            Statement::Expr(expr) => self.expr(expr),
            Statement::VocabExpressionItem(item) => {
                self.expr(&item.expr);
                for modifier in &item.modifiers {
                    self.expr(&modifier.value);
                }
            }
            Statement::Surface(surface) => match &surface.payload {
                SurfaceStmtPayload::KeywordArgs(args) => {
                    for arg in args {
                        self.expr(arg);
                    }
                }
            },
            // A vocabulary block is desugared before the checker runs.
            Statement::Pass | Statement::Continue | Statement::VocabBlock(_) => {}
        }
    }

    /// Walk an `if`/`while` condition, including the pattern of an `if let`/`while let`.
    fn condition(&mut self, condition: &Condition) {
        match condition {
            Condition::Expr(expr) => self.expr(expr),
            Condition::Let { pattern, value } => {
                self.pattern(pattern);
                self.expr(value);
            }
        }
    }

    /// Walk the names a pattern binds.
    fn pattern(&mut self, pattern: &Spanned<Pattern>) {
        match &pattern.node {
            Pattern::Binding(name) => self.check(name, "binding", pattern.span),
            Pattern::Constructor(_, args) => {
                for arg in args {
                    match arg {
                        PatternArg::Positional(inner) | PatternArg::Named(_, inner) => self.pattern(inner),
                    }
                }
            }
            Pattern::Tuple(items) | Pattern::Or(items) => {
                for item in items {
                    self.pattern(item);
                }
            }
            Pattern::Group(inner) => self.pattern(inner),
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
    }

    // ---- Expressions ----

    /// Walk an expression for the bindings nested inside it: closure parameters, comprehension and match patterns,
    /// and the statements of expression-position blocks.
    fn expr(&mut self, expr: &Spanned<Expr>) {
        match &expr.node {
            Expr::Closure(params, body) => {
                self.params(params);
                self.expr(body);
            }
            Expr::Match(subject, arms) => {
                self.expr(subject);
                for arm in arms {
                    self.pattern(&arm.node.pattern);
                    if let Some(guard) = &arm.node.guard {
                        self.expr(guard);
                    }
                    match &arm.node.body {
                        MatchBody::Expr(body) => self.expr(body),
                        MatchBody::Block(body) => self.block(body),
                    }
                }
            }
            Expr::ListComp(comp) => {
                self.pattern(&comp.pattern);
                self.expr(&comp.iter);
                if let Some(filter) = &comp.filter {
                    self.expr(filter);
                }
                self.clauses(&comp.clauses);
                self.expr(&comp.expr);
            }
            Expr::DictComp(comp) => {
                self.pattern(&comp.pattern);
                self.expr(&comp.iter);
                if let Some(filter) = &comp.filter {
                    self.expr(filter);
                }
                self.clauses(&comp.clauses);
                self.expr(&comp.key);
                self.expr(&comp.value);
            }
            Expr::Generator(generator) => {
                self.clauses(&generator.clauses);
                self.expr(&generator.expr);
            }
            Expr::If(if_expr) => {
                self.expr(&if_expr.condition);
                self.block(&if_expr.then_body);
                if let Some(else_body) = &if_expr.else_body {
                    self.block(else_body);
                }
            }
            Expr::Loop(loop_expr) => self.block(&loop_expr.body),
            Expr::Binary(left, _, right) | Expr::Index(left, right) => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Field(inner, _) => self.expr(inner),
            Expr::Yield(inner) => {
                if let Some(inner) = inner {
                    self.expr(inner);
                }
            }
            Expr::Call(callee, _, args) | Expr::MethodCall(callee, _, _, args) => {
                self.expr(callee);
                self.call_args(args);
            }
            Expr::Constructor(_, args) => self.call_args(args),
            Expr::Partial(partial) => {
                self.expr(&partial.target);
                for arg in &partial.args {
                    self.expr(&arg.value);
                }
            }
            Expr::Slice(target, slice) => {
                self.expr(target);
                for bound in [&slice.start, &slice.end, &slice.step].into_iter().flatten() {
                    self.expr(bound);
                }
            }
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.expr(item);
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(item) | ListEntry::Spread(item) => self.expr(item),
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.expr(key);
                            self.expr(value);
                        }
                        DictEntry::Spread(item) => self.expr(item),
                    }
                }
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.expr(expr);
                    }
                }
            }
            Expr::Surface(surface) => self.surface(&surface.payload),
            Expr::Embedded(fragment) => self.embedded(fragment),
            // Leaves bind nothing, and a vocabulary block is desugared before the checker runs.
            Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr | Expr::VocabBlock(_) => {}
        }
    }

    /// Walk call arguments.
    fn call_args(&mut self, args: &[CallArg]) {
        for arg in args {
            match arg {
                CallArg::Positional(value)
                | CallArg::Named(_, value)
                | CallArg::PositionalUnpack(value)
                | CallArg::KeywordUnpack(value) => self.expr(value),
            }
        }
    }

    /// Walk the `for` patterns, iterables and filters of a comprehension.
    fn clauses(&mut self, clauses: &[ComprehensionClause]) {
        for clause in clauses {
            match clause {
                ComprehensionClause::For { pattern, iter } => {
                    self.pattern(pattern);
                    self.expr(iter);
                }
                ComprehensionClause::If(condition) => self.expr(condition),
            }
        }
    }

    /// Walk a surface expression's payload, including the winner binding of a `race for` block.
    fn surface(&mut self, payload: &SurfaceExprPayload) {
        match payload {
            SurfaceExprPayload::PrefixUnary(inner) => self.expr(inner),
            SurfaceExprPayload::RaceFor(race) => {
                self.check(&race.binding.node, "binding", race.binding.span);
                for arm in &race.arms {
                    self.expr(&arm.awaitable);
                    match &arm.body {
                        RaceForBody::Expr(body) => self.expr(body),
                        RaceForBody::Block(body) => self.block(body),
                    }
                }
            }
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            SurfaceExprPayload::ScopedSymbolCall { args, .. } => self.call_args(args),
            SurfaceExprPayload::LeadingDotPath { .. } => {}
        }
    }

    /// Walk the ordinary Incan expressions an embedded fragment owns.
    fn embedded(&mut self, fragment: &EmbeddedFragmentExpr) {
        for hole in fragment.holes() {
            self.expr(hole);
        }
    }
}

impl TypeChecker {
    /// Refuse every name the program declares or binds with the compiler's reserved `__incan_` prefix (#1769).
    ///
    /// Runs once per program check, before collection, over the program's own declarations; dependency modules are
    /// refused when they are checked as programs of their own. Each refusal carries `INCAN-T0111` and the position
    /// of the declaration, parameter or binding that spells the name.
    pub(super) fn report_reserved_compiler_names(&mut self, program: &Program) {
        let mut walk = ReservedNameWalk {
            refusals: Vec::new(),
            reported: HashSet::new(),
        };
        for declaration in &program.declarations {
            walk.declaration(declaration);
        }
        self.errors.extend(walk.refusals);
    }
}
