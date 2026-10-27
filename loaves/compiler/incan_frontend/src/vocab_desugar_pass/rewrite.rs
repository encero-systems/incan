use crate::ast;
use crate::diagnostics::CompileError;
use crate::loaf_index::LoafIndex;
use crate::vocab_ast_bridge::{internal_vocab_block_to_public, public_declarations_to_internal_with_anchor};

use super::surface::Output;
use super::{NativeDesugarerRuntime, VocabDesugarPassError};

/// Rewrite all raw vocab blocks in a parsed program before typechecking, so downstream phases operate only on
/// ordinary compiler declarations, statements, and expressions.
///
/// The module is rewritten in place, in at most two passes of the same traversal, so declaration, expression and
/// statement ownership cannot drift:
/// 1. rewrite every node that needs no provider reply, keeping each provider node and collecting its request;
/// 2. invoke each provider once for the collected requests;
/// 3. when a node was kept, rewrite the kept nodes from the validated replies;
/// 4. write the module's evidence once.
///
/// # Errors
///
/// Returns compiler diagnostics when AST bridge mapping fails, desugarer artifact resolution fails, native worker
/// execution fails, or desugarer output cannot be decoded and mapped back into internal AST.
pub fn desugar_program_vocab_blocks_with_runtime(
    program: &mut ast::Program,
    module_path: Option<&str>,
    loaves: &LoafIndex,
    runtime: &mut NativeDesugarerRuntime,
) -> Result<(), Vec<CompileError>> {
    // 1. Rewrite what needs no provider, keeping provider nodes and collecting their requests.
    runtime.begin_module();
    let mut rewrite = Rewrite {
        module_path,
        loaves,
        runtime,
        errors: Vec::new(),
    };
    rewrite.declaration_list(&mut program.declarations);
    // 2. and 3. Invoke the providers, then rewrite the kept nodes; a module without provider nodes is done.
    if rewrite.runtime.finish_module(module_path.unwrap_or("")) {
        rewrite.declaration_list(&mut program.declarations);
    }
    // 4. Write the module's evidence.
    rewrite.runtime.flush_evidence();
    if rewrite.errors.is_empty() {
        Ok(())
    } else {
        Err(rewrite.errors)
    }
}

/// One module's rewrite: the provider runtime and the node-local diagnostics of both passes.
struct Rewrite<'a> {
    /// The requesting module's path, carried in provider requests and evidence.
    module_path: Option<&'a str>,
    /// The compiled Loaves whose vocabulary the parser activated.
    loaves: &'a LoafIndex,
    /// Collects provider requests in the first pass and answers them in the second.
    runtime: &'a mut NativeDesugarerRuntime,
    /// Node-local diagnostics; the rest of the module is still rewritten.
    errors: Vec<CompileError>,
}

impl Rewrite<'_> {
    /// Rewrite one declaration list, replacing top-level vocabulary declarations with ordinary lowered declarations.
    fn declaration_list(&mut self, declarations: &mut Vec<ast::Spanned<ast::Declaration>>) {
        let mut rewritten = Vec::new();
        for declaration in std::mem::take(declarations) {
            let span = declaration.span;
            let leading_blank_lines = declaration.leading_blank_lines;
            let required_features = declaration.required_features.clone();
            match declaration.node {
                ast::Declaration::VocabBlock(block) => {
                    let mut lowered = match self.vocab_declaration(&block, span) {
                        Ok(Some(declarations)) => declarations,
                        Ok(None) => {
                            rewritten.push(ast::Spanned {
                                node: ast::Declaration::VocabBlock(block),
                                span,
                                leading_blank_lines,
                                required_features,
                            });
                            continue;
                        }
                        Err(error) => {
                            self.errors.push(error);
                            continue;
                        }
                    };
                    for (index, declaration) in lowered.iter_mut().enumerate() {
                        declaration.leading_blank_lines = if index == 0 { leading_blank_lines } else { 0 };
                        declaration.required_features.clone_from(&required_features);
                        self.declaration(&mut declaration.node);
                    }
                    rewritten.extend(lowered);
                }
                mut declaration => {
                    self.declaration(&mut declaration);
                    rewritten.push(ast::Spanned {
                        node: declaration,
                        span,
                        leading_blank_lines,
                        required_features,
                    });
                }
            }
        }
        *declarations = rewritten;
    }

    /// Desugar one top-level vocabulary declaration into ordinary compiler declarations.
    ///
    /// Returns `Ok(None)` while the runtime collects: the caller keeps the declaration for the second pass.
    fn vocab_declaration(
        &mut self,
        block: &ast::VocabBlockStmt,
        span: ast::Span,
    ) -> Result<Option<Vec<ast::Spanned<ast::Declaration>>>, CompileError> {
        let bridged = internal_vocab_block_to_public(block, span).map_err(|source| {
            error_from_pass_error(
                VocabDesugarPassError::Bridge {
                    keyword: block.keyword.clone(),
                    source,
                },
                span,
            )
        })?;
        if is_standard_c_binding(&bridged, self.loaves) {
            let declarations =
                super::checked_c::binding(&bridged).map_err(|message| CompileError::syntax(message, span))?;
            return public_declarations_to_internal_with_anchor(&declarations, span)
                .map(Some)
                .map_err(|error| CompileError::syntax(format!("invalid checked C binding: {error}"), span));
        }
        match self.desugar(block, span)? {
            None => Ok(None),
            Some(Output::Declarations(declarations)) => Ok(Some(declarations)),
            Some(_) => Err(unsupported_output(block, span)),
        }
    }

    /// Ask the runtime for one node's validated output, or `None` while it collects requests.
    fn desugar(&mut self, block: &ast::VocabBlockStmt, span: ast::Span) -> Result<Option<Output>, CompileError> {
        self.runtime
            .desugar_node(self.loaves, block, span, self.module_path)
            .map_err(|error| error_from_pass_error(error, span))
    }

    /// Rewrite vocab blocks inside every expression-bearing surface of one declaration.
    fn declaration(&mut self, declaration: &mut ast::Declaration) {
        match declaration {
            ast::Declaration::Const(konst) => self.expr(&mut konst.value),
            ast::Declaration::Static(static_decl) => self.expr(&mut static_decl.value),
            ast::Declaration::Partial(partial) => self.partial_args(&mut partial.args),
            ast::Declaration::Function(function) => self.statement_list(&mut function.body),
            ast::Declaration::Model(model) => {
                self.field_defaults(&mut model.fields);
                self.method_partials(&mut model.method_partials);
                for property in &mut model.properties {
                    self.optional_body(property.node.body.as_mut());
                }
                for method in &mut model.methods {
                    self.optional_body(method.node.body.as_mut());
                }
            }
            ast::Declaration::Class(class) => {
                self.field_defaults(&mut class.fields);
                self.method_partials(&mut class.method_partials);
                for property in &mut class.properties {
                    self.optional_body(property.node.body.as_mut());
                }
                for method in &mut class.methods {
                    self.optional_body(method.node.body.as_mut());
                }
            }
            ast::Declaration::Trait(trait_decl) => {
                self.method_partials(&mut trait_decl.method_partials);
                for property in &mut trait_decl.properties {
                    self.optional_body(property.node.body.as_mut());
                }
                for method in &mut trait_decl.methods {
                    self.optional_body(method.node.body.as_mut());
                }
            }
            ast::Declaration::Newtype(newtype_decl) => {
                self.method_partials(&mut newtype_decl.method_partials);
                for method in &mut newtype_decl.methods {
                    self.optional_body(method.node.body.as_mut());
                }
            }
            ast::Declaration::TestModule(test_module) => self.declaration_list(&mut test_module.body),
            _ => {}
        }
    }

    /// Rewrite a method or property body when the declaration has one.
    fn optional_body(&mut self, body: Option<&mut Vec<ast::Spanned<ast::Statement>>>) {
        if let Some(body) = body {
            self.statement_list(body);
        }
    }

    /// Rewrite vocab expressions in model or class field default values.
    fn field_defaults(&mut self, fields: &mut [ast::Spanned<ast::FieldDecl>]) {
        for field in fields {
            if let Some(default) = field.node.default.as_mut() {
                self.expr(default);
            }
        }
    }

    /// Rewrite vocab expressions inside method-level partial preset arguments.
    fn method_partials(&mut self, partials: &mut [ast::Spanned<ast::MethodPartialDecl>]) {
        for partial in partials {
            self.partial_args(&mut partial.node.args);
        }
    }

    /// Rewrite vocab expressions inside one partial preset argument list.
    fn partial_args(&mut self, args: &mut [ast::PartialArg]) {
        for arg in args {
            self.expr(&mut arg.value);
        }
    }

    /// Recursively rewrite a statement list so no raw vocab block statement or expression nodes survive this pass.
    ///
    /// The recursion matters because desugarers may emit statements that themselves still contain nested control-flow
    /// bodies, and those bodies may contain additional vocab blocks introduced earlier by parsing.
    fn statement_list(&mut self, statements: &mut Vec<ast::Spanned<ast::Statement>>) {
        let mut rewritten = Vec::new();
        for statement in statements.drain(..) {
            let span = statement.span;
            match statement.node {
                // Without a lowering hook, preserve RFC 081's existing owned-fragment path, including ordinary checking
                // of expression holes and the explicit emission refusal. A registered hook receives the structured
                // artifact through the native worker instead.
                ast::Statement::VocabBlock(block)
                    if vocab_block_body_is_embedded_fragment(&block) && !fragment_has_hook(&block, self.loaves) =>
                {
                    let mut body = block.body;
                    self.statement_list(&mut body);
                    rewritten.extend(body);
                }
                ast::Statement::VocabBlock(block) => rewritten.extend(self.vocab_statement(block, span)),
                mut statement => {
                    self.statement(&mut statement);
                    rewritten.push(ast::Spanned::new(statement, span));
                }
            }
        }
        *statements = rewritten;
    }

    /// Rewrite one statement-position vocabulary block into its statements; a collected block is kept as it is.
    fn vocab_statement(&mut self, block: ast::VocabBlockStmt, span: ast::Span) -> Vec<ast::Spanned<ast::Statement>> {
        let desugared = match self.desugar(&block, span) {
            Ok(Some(output)) => output,
            Ok(None) => return vec![ast::Spanned::new(ast::Statement::VocabBlock(block), span)],
            Err(error) => {
                self.errors.push(error);
                return Vec::new();
            }
        };
        let mut lowered = match desugared {
            Output::Statements(statements) => statements,
            Output::Expression(expression) => vec![ast::Spanned::new(ast::Statement::Expr(expression), span)],
            Output::Declarations(_) => {
                self.errors.push(CompileError::syntax(
                    "declaration output requires compilation-unit scope".into(),
                    span,
                ));
                return Vec::new();
            }
        };
        self.statement_list(&mut lowered);
        lowered
    }

    /// Rewrite the expressions and nested bodies of one ordinary statement.
    fn statement(&mut self, statement: &mut ast::Statement) {
        match statement {
            ast::Statement::Assignment(assignment) => self.expr(&mut assignment.value),
            ast::Statement::FieldAssignment(assignment) => {
                self.expr(&mut assignment.object);
                self.expr(&mut assignment.value);
            }
            ast::Statement::IndexAssignment(assignment) => {
                self.expr(&mut assignment.object);
                self.expr(&mut assignment.index);
                self.expr(&mut assignment.value);
            }
            ast::Statement::Return(Some(expr)) | ast::Statement::Break(Some(expr)) => self.expr(expr),
            ast::Statement::If(if_stmt) => {
                self.condition(&mut if_stmt.condition);
                self.statement_list(&mut if_stmt.then_body);
                for (elif_condition, elif_body) in &mut if_stmt.elif_branches {
                    self.expr(elif_condition);
                    self.statement_list(elif_body);
                }
                if let Some(else_body) = if_stmt.else_body.as_mut() {
                    self.statement_list(else_body);
                }
            }
            ast::Statement::While(while_stmt) => {
                self.condition(&mut while_stmt.condition);
                self.statement_list(&mut while_stmt.body);
            }
            ast::Statement::Loop(loop_stmt) => self.statement_list(&mut loop_stmt.body),
            ast::Statement::For(for_stmt) => {
                self.expr(&mut for_stmt.iter);
                self.statement_list(&mut for_stmt.body);
            }
            ast::Statement::Expr(expr) => self.expr(expr),
            ast::Statement::VocabExpressionItem(item) => {
                self.expr(&mut item.expr);
                for modifier in &mut item.modifiers {
                    self.expr(&mut modifier.value);
                }
            }
            ast::Statement::Assert(assert_stmt) => self.assert(assert_stmt),
            ast::Statement::CompoundAssignment(assignment) => self.expr(&mut assignment.value),
            ast::Statement::TupleUnpack(assignment) => self.expr(&mut assignment.value),
            ast::Statement::TupleAssign(assignment) => {
                for target in &mut assignment.targets {
                    self.expr(target);
                }
                self.expr(&mut assignment.value);
            }
            ast::Statement::ChainedAssignment(assignment) => self.expr(&mut assignment.value),
            _ => {}
        }
    }

    /// Rewrite vocab expressions inside conditional expression wrappers.
    fn condition(&mut self, condition: &mut ast::Condition) {
        match condition {
            ast::Condition::Expr(expr) | ast::Condition::Let { value: expr, .. } => self.expr(expr),
        }
    }

    /// Rewrite vocab expressions inside all assert statement payloads.
    fn assert(&mut self, assert_stmt: &mut ast::AssertStmt) {
        match &mut assert_stmt.kind {
            ast::AssertKind::Condition(expr) => self.expr(expr),
            ast::AssertKind::IsPattern { value, .. } => self.expr(value),
            ast::AssertKind::Raises { call, .. } => self.expr(call),
        }
        if let Some(message) = assert_stmt.message.as_mut() {
            self.expr(message);
        }
    }

    /// Recursively rewrite raw vocab expression declarations nested inside ordinary expressions.
    fn expr(&mut self, expr: &mut ast::Spanned<ast::Expr>) {
        if matches!(expr.node, ast::Expr::VocabBlock(_)) && !self.vocab_expr(expr) {
            return;
        }
        match &mut expr.node {
            ast::Expr::Binary(left, _, right) => {
                self.expr(left);
                self.expr(right);
            }
            ast::Expr::Unary(_, inner)
            | ast::Expr::Try(inner)
            | ast::Expr::Paren(inner)
            | ast::Expr::Yield(Some(inner)) => self.expr(inner),
            ast::Expr::Call(callee, _, args) => {
                self.expr(callee);
                self.call_args(args);
            }
            ast::Expr::Index(base, index) => {
                self.expr(base);
                self.expr(index);
            }
            ast::Expr::Slice(base, slice) => {
                self.expr(base);
                for bound in [&mut slice.start, &mut slice.end, &mut slice.step]
                    .into_iter()
                    .flatten()
                {
                    self.expr(bound);
                }
            }
            ast::Expr::Field(base, _) => self.expr(base),
            ast::Expr::MethodCall(receiver, _, _, args) => {
                self.expr(receiver);
                self.call_args(args);
            }
            ast::Expr::Partial(partial) => {
                self.expr(&mut partial.target);
                for arg in &mut partial.args {
                    self.expr(&mut arg.value);
                }
            }
            ast::Expr::Match(scrutinee, arms) => {
                self.expr(scrutinee);
                self.match_arms(arms);
            }
            ast::Expr::If(if_expr) => {
                self.expr(&mut if_expr.condition);
                self.statement_list(&mut if_expr.then_body);
                if let Some(else_body) = if_expr.else_body.as_mut() {
                    self.statement_list(else_body);
                }
            }
            ast::Expr::Loop(loop_expr) => self.statement_list(&mut loop_expr.body),
            ast::Expr::ListComp(comp) => {
                self.expr(&mut comp.expr);
                self.expr(&mut comp.iter);
                if let Some(filter) = comp.filter.as_mut() {
                    self.expr(filter);
                }
                self.comprehension_clauses(&mut comp.clauses);
            }
            ast::Expr::DictComp(comp) => {
                self.expr(&mut comp.key);
                self.expr(&mut comp.value);
                self.expr(&mut comp.iter);
                if let Some(filter) = comp.filter.as_mut() {
                    self.expr(filter);
                }
                self.comprehension_clauses(&mut comp.clauses);
            }
            ast::Expr::Generator(generator) => {
                self.expr(&mut generator.expr);
                self.comprehension_clauses(&mut generator.clauses);
            }
            ast::Expr::Closure(_, body) => self.expr(body),
            ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
                for item in items {
                    self.expr(item);
                }
            }
            ast::Expr::List(items) => {
                for item in items {
                    match item {
                        ast::ListEntry::Element(expr) | ast::ListEntry::Spread(expr) => self.expr(expr),
                    }
                }
            }
            ast::Expr::Dict(entries) => self.dict_entries(entries),
            ast::Expr::Constructor(_, args) => self.call_args(args),
            ast::Expr::FString(parts) => {
                for part in parts {
                    if let ast::FStringPart::Expr { expr, .. } = part {
                        self.expr(expr);
                    }
                }
            }
            ast::Expr::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
            }
            ast::Expr::Surface(surface) => self.surface_expr(surface),
            // RFC 081 (#1023): unlike `VocabBlock`/`Surface` above, the embedded-fragment container itself is *not*
            // erased or replaced by this pass -- it must survive into typecheck/lowering as itself so its holes get
            // real Incan typing (see `EmbeddedFragmentExpr`'s rustdoc). Its expression holes are still recursed into
            // here, exactly like `Surface`'s operands above, so a hole that itself uses some other DSL's vocab block
            // or scoped surface still gets desugared correctly.
            ast::Expr::Embedded(fragment) => {
                for hole in fragment.holes_mut() {
                    self.expr(hole);
                }
            }
            ast::Expr::Ident(_)
            | ast::Expr::Literal(_)
            | ast::Expr::SelfExpr
            | ast::Expr::Yield(None)
            | ast::Expr::VocabBlock(_) => {}
        }
    }

    /// Rewrite each guard before its body, retaining the arm's pattern and source metadata.
    fn match_arms(&mut self, arms: &mut [ast::Spanned<ast::MatchArm>]) {
        for arm in arms {
            if let Some(guard) = arm.node.guard.as_mut() {
                self.expr(guard);
            }
            match &mut arm.node.body {
                ast::MatchBody::Expr(body) => self.expr(body),
                ast::MatchBody::Block(body) => self.statement_list(body),
            }
        }
    }

    /// Rewrite dictionary keys before values and spreads in their written order.
    fn dict_entries(&mut self, entries: &mut [ast::DictEntry]) {
        for entry in entries {
            match entry {
                ast::DictEntry::Pair(key, value) => {
                    self.expr(key);
                    self.expr(value);
                }
                ast::DictEntry::Spread(value) => self.expr(value),
            }
        }
    }

    /// Replace one expression-position vocabulary block by its expansion.
    ///
    /// Returns whether the expression should be rewritten further: false for a block kept for the second pass and
    /// for a refused block, which leaves a `None` placeholder beside its diagnostic.
    fn vocab_expr(&mut self, expr: &mut ast::Spanned<ast::Expr>) -> bool {
        let placeholder = ast::Expr::Literal(ast::Literal::None);
        let block = match std::mem::replace(&mut expr.node, placeholder) {
            ast::Expr::VocabBlock(block) => block,
            other => {
                expr.node = other;
                return true;
            }
        };
        match self.vocab_expression(&block, expr.span) {
            Ok(Some(node)) => {
                *expr = node;
                true
            }
            Ok(None) => {
                expr.node = ast::Expr::VocabBlock(block);
                false
            }
            Err(error) => {
                self.errors.push(error);
                false
            }
        }
    }

    /// Desugar one expression-position vocab block and convert the expression result back to compiler AST.
    ///
    /// Returns `Ok(None)` while the runtime collects: the caller keeps the block for the second pass.
    fn vocab_expression(
        &mut self,
        block: &ast::VocabBlockStmt,
        span: ast::Span,
    ) -> Result<Option<ast::Spanned<ast::Expr>>, CompileError> {
        match self.desugar(block, span)? {
            None => Ok(None),
            Some(Output::Expression(expression)) => Ok(Some(expression)),
            Some(_) => Err(unsupported_output(block, span)),
        }
    }

    /// Rewrite vocab expressions inside positional, named, and unpacked call arguments.
    fn call_args(&mut self, args: &mut [ast::CallArg]) {
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr)
                | ast::CallArg::Named(_, expr)
                | ast::CallArg::PositionalUnpack(expr)
                | ast::CallArg::KeywordUnpack(expr) => self.expr(expr),
            }
        }
    }

    /// Rewrite vocab expressions inside comprehension iterator and filter clauses.
    fn comprehension_clauses(&mut self, clauses: &mut [ast::ComprehensionClause]) {
        for clause in clauses {
            match clause {
                ast::ComprehensionClause::For { iter, .. } | ast::ComprehensionClause::If(iter) => self.expr(iter),
            }
        }
    }

    /// Rewrite nested expressions held by non-core surface-expression payloads.
    fn surface_expr(&mut self, surface: &mut ast::SurfaceExpr) {
        match &mut surface.payload {
            ast::SurfaceExprPayload::PrefixUnary(inner) => self.expr(inner),
            ast::SurfaceExprPayload::RaceFor(race) => {
                for arm in &mut race.arms {
                    self.expr(&mut arm.awaitable);
                    match &mut arm.body {
                        ast::RaceForBody::Expr(expr) => self.expr(expr),
                        ast::RaceForBody::Block(body) => self.statement_list(body),
                    }
                }
            }
            ast::SurfaceExprPayload::LeadingDotPath { .. } => {}
            ast::SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ast::SurfaceExprPayload::ScopedSymbolCall { args, .. } => self.call_args(args),
        }
    }
}

/// Select compiler-owned C binding semantics only for the pinned standard namespace issuer.
fn is_standard_c_binding(declaration: &incan_vocab::VocabDeclaration, loaves: &LoafIndex) -> bool {
    if declaration.keyword != "binding" {
        return false;
    }
    let Some(metadata) = &declaration.keyword_metadata else {
        return false;
    };
    loaves.entry(&metadata.dependency_key).is_some_and(|entry| {
        matches!(&entry.namespace, crate::loaf_index::LoafNamespace::Standard { policy }
            if policy.namespace_roots.contains(&incan_lang::lang::stdlib::STDLIB_INTEROP))
    })
}

/// Return whether a vocab block's body is exactly the shape `Parser::try_embedded_fragment_body` produces (RFC
/// 081, `#1023`): a single expression statement wrapping `Expr::Embedded`.
///
/// This is the parser's only way to produce that shape (`try_vocab_block` has one call site,
/// `try_vocab_block_statement`, which always wraps the result in `Statement::VocabBlock`), so checking the body
/// shape here is a reliable, purely structural test -- no descriptor lookup is needed at this stage of the
/// pipeline.
fn vocab_block_body_is_embedded_fragment(block: &ast::VocabBlockStmt) -> bool {
    matches!(
        block.body.as_slice(),
        [ast::Spanned {
            node: ast::Statement::Expr(ast::Spanned {
                node: ast::Expr::Embedded(_),
                ..
            }),
            ..
        }]
    )
}

/// Preserve RFC 081's no-hook route only when the owning published vocabulary supplies no native desugarer.
fn fragment_has_hook(block: &ast::VocabBlockStmt, loaves: &LoafIndex) -> bool {
    loaves
        .entry(&block.keyword_binding.dependency_key)
        .and_then(|entry| entry.artifact.manifest.vocab.as_ref())
        .is_some_and(|vocab| vocab.desugarer_artifact.is_some())
}

/// The refusal for a provider output that cannot occupy the block's source position.
fn unsupported_output(block: &ast::VocabBlockStmt, span: ast::Span) -> CompileError {
    error_from_pass_error(
        VocabDesugarPassError::UnsupportedOutput {
            keyword: block.keyword.clone(),
        },
        span,
    )
}

/// Map a pass/runtime error into a compiler diagnostic.
///
/// These are infrastructure failures (artifact I/O, worker execution, checksum mismatches), not semantic type errors,
/// so we use the generic `Error` kind rather than `Type`.
fn error_from_pass_error(error: VocabDesugarPassError, fallback_span: ast::Span) -> CompileError {
    let span = match &error {
        VocabDesugarPassError::Native { span: Some(span), .. } => *span,
        _ => fallback_span,
    };
    CompileError::new(format!("vocab desugar pass failed: {error}"), span)
}
