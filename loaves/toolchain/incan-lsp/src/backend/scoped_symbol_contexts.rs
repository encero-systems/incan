//! Active scoped symbol completions and the vocab context at an offset.

use super::*;

/// Return scoped symbol descriptors eligible for completion at the given document offset.
pub(super) fn active_scoped_symbol_completions<'a>(
    ast: &'a Program,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    offset: usize,
) -> Vec<ScopedSymbolLspDescriptor<'a>> {
    let Some(context) = scoped_symbol_context_at_offset(ast, offset) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for (dependency_key, surface) in active_imported_dsl_surfaces(ast, surfaces, offset) {
        for descriptor in &surface.scoped_symbols {
            let Some(depth) = scoped_symbol_depth_in_context(descriptor, &context) else {
                continue;
            };
            candidates.push(ScopedSymbolLspDescriptor {
                dependency_key,
                descriptor,
                depth,
            });
        }
    }

    let mut max_depth_by_symbol: HashMap<&str, usize> = HashMap::new();
    for candidate in &candidates {
        max_depth_by_symbol
            .entry(candidate.descriptor.symbol.as_str())
            .and_modify(|depth| *depth = (*depth).max(candidate.depth))
            .or_insert(candidate.depth);
    }
    candidates
        .into_iter()
        .filter(|candidate| {
            max_depth_by_symbol
                .get(candidate.descriptor.symbol.as_str())
                .is_some_and(|depth| *depth == candidate.depth)
        })
        .collect()
}

/// Return DSL surfaces activated by `pub::` imports before the given document offset.
pub(super) fn active_imported_dsl_surfaces<'a>(
    ast: &'a Program,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    offset: usize,
) -> Vec<(&'a str, &'a incan_vocab::DslSurface)> {
    let mut active = Vec::new();
    for decl in &ast.declarations {
        if decl.span.end > offset {
            break;
        }
        let Declaration::Import(import) = &decl.node else {
            continue;
        };
        let library = match &import.kind {
            incan_frontend::ast::ImportKind::PubLibrary { library, .. }
            | incan_frontend::ast::ImportKind::PubFrom { library, .. } => library.as_str(),
            _ => continue,
        };
        let Some(library_surfaces) = surfaces.get(library) else {
            continue;
        };
        active.extend(
            library_surfaces
                .iter()
                .filter(move |surface| dsl_surface_applies_to_pub_import_lsp(surface, library))
                .map(move |surface| (library, surface)),
        );
    }
    active
}

/// Return whether an imported DSL surface activates for a `pub::library` import in LSP helpers.
pub(super) fn dsl_surface_applies_to_pub_import_lsp(surface: &incan_vocab::DslSurface, library: &str) -> bool {
    match &surface.activation {
        incan_vocab::KeywordActivation::Always => true,
        incan_vocab::KeywordActivation::OnImport { namespace } => namespace_matches_pub_library_lsp(namespace, library),
        _ => false,
    }
}

/// Return whether an activation namespace matches a `pub::library` import in LSP helpers.
pub(super) fn namespace_matches_pub_library_lsp(namespace: &str, library: &str) -> bool {
    let trimmed = namespace.trim();
    !trimmed.is_empty() && (trimmed == library || trimmed.starts_with(&format!("{library}.")))
}

/// Return the innermost lexical DSL depth where a descriptor is eligible in the current LSP context.
pub(super) fn scoped_symbol_depth_in_context(
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
    context: &ScopedSymbolLspContext,
) -> Option<usize> {
    descriptor
        .eligible_in
        .iter()
        .filter_map(|eligibility| scoped_symbol_eligibility_depth(eligibility, context))
        .max()
}

/// Return the lexical DSL depth where one eligibility rule matches the current LSP context.
pub(super) fn scoped_symbol_eligibility_depth(
    eligibility: &incan_vocab::ScopedSymbolEligibility,
    context: &ScopedSymbolLspContext,
) -> Option<usize> {
    match eligibility.position {
        incan_vocab::ScopedSymbolPosition::DeclarationBody => context
            .vocab_stack
            .iter()
            .rposition(|declaration| declaration == &eligibility.declaration),
        incan_vocab::ScopedSymbolPosition::ClauseBody => {
            let clause = eligibility.clause.as_deref()?;
            let clause_depth = context.vocab_stack.iter().rposition(|active| active == clause)?;
            context
                .vocab_stack
                .iter()
                .take(clause_depth)
                .rposition(|active| active == &eligibility.declaration)
                .map(|declaration_depth| declaration_depth.max(clause_depth))
        }
        incan_vocab::ScopedSymbolPosition::CallArgument => {
            if eligibility.call.as_deref() != context.call_target.as_deref() {
                return None;
            }
            context
                .vocab_stack
                .iter()
                .rposition(|declaration| declaration == &eligibility.declaration)
        }
        _ => None,
    }
}

/// Derive the DSL block stack and call-argument target active at a document offset.
pub(super) fn scoped_symbol_context_at_offset(ast: &Program, offset: usize) -> Option<ScopedSymbolLspContext> {
    let mut context = ScopedSymbolLspContext {
        vocab_stack: Vec::new(),
        call_target: None,
    };
    for decl in &ast.declarations {
        if decl.span.start <= offset && offset <= decl.span.end {
            scoped_symbol_context_in_declaration(decl, offset, &mut context);
        }
    }
    if context.vocab_stack.is_empty() {
        None
    } else {
        Some(context)
    }
}

/// Update scoped symbol completion context from one declaration containing the offset.
pub(super) fn scoped_symbol_context_in_declaration(
    decl: &Spanned<Declaration>,
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    match &decl.node {
        Declaration::Const(konst) => scoped_symbol_context_in_expr(&konst.value, offset, context),
        Declaration::Static(static_decl) => scoped_symbol_context_in_expr(&static_decl.value, offset, context),
        Declaration::Function(func) => scoped_symbol_context_in_statements(&func.body, offset, context),
        Declaration::Model(model) => {
            for method in &model.methods {
                if let Some(body) = &method.node.body {
                    scoped_symbol_context_in_statements(body, offset, context);
                }
            }
            for property in &model.properties {
                if let Some(body) = &property.node.body {
                    scoped_symbol_context_in_statements(body, offset, context);
                }
            }
        }
        Declaration::Class(class) => {
            for method in &class.methods {
                if let Some(body) = &method.node.body {
                    scoped_symbol_context_in_statements(body, offset, context);
                }
            }
            for property in &class.properties {
                if let Some(body) = &property.node.body {
                    scoped_symbol_context_in_statements(body, offset, context);
                }
            }
        }
        Declaration::Trait(trait_decl) => {
            for method in &trait_decl.methods {
                if let Some(body) = &method.node.body {
                    scoped_symbol_context_in_statements(body, offset, context);
                }
            }
            for property in &trait_decl.properties {
                if let Some(body) = &property.node.body {
                    scoped_symbol_context_in_statements(body, offset, context);
                }
            }
        }
        _ => {}
    }
}

/// Update scoped symbol completion context from a statement list containing the offset.
pub(super) fn scoped_symbol_context_in_statements(
    statements: &[Spanned<Statement>],
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    for stmt in statements {
        if stmt.span.start <= offset && offset <= stmt.span.end {
            scoped_symbol_context_in_statement(stmt, offset, context);
        }
    }
}

/// Update scoped symbol completion context from one statement containing the offset.
pub(super) fn scoped_symbol_context_in_statement(
    stmt: &Spanned<Statement>,
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    match &stmt.node {
        Statement::Assignment(assign) => scoped_symbol_context_in_expr(&assign.value, offset, context),
        Statement::FieldAssignment(assign) => {
            scoped_symbol_context_in_expr(&assign.object, offset, context);
            scoped_symbol_context_in_expr(&assign.value, offset, context);
        }
        Statement::IndexAssignment(assign) => {
            scoped_symbol_context_in_expr(&assign.object, offset, context);
            scoped_symbol_context_in_expr(&assign.index, offset, context);
            scoped_symbol_context_in_expr(&assign.value, offset, context);
        }
        Statement::Return(expr) | Statement::Break(expr) => {
            if let Some(expr) = expr {
                scoped_symbol_context_in_expr(expr, offset, context);
            }
        }
        Statement::If(if_stmt) => {
            scoped_symbol_context_in_condition(&if_stmt.condition, offset, context);
            scoped_symbol_context_in_statements(&if_stmt.then_body, offset, context);
            for (condition, body) in &if_stmt.elif_branches {
                scoped_symbol_context_in_expr(condition, offset, context);
                scoped_symbol_context_in_statements(body, offset, context);
            }
            if let Some(body) = &if_stmt.else_body {
                scoped_symbol_context_in_statements(body, offset, context);
            }
        }
        Statement::Loop(loop_stmt) => scoped_symbol_context_in_statements(&loop_stmt.body, offset, context),
        Statement::While(while_stmt) => {
            scoped_symbol_context_in_condition(&while_stmt.condition, offset, context);
            scoped_symbol_context_in_statements(&while_stmt.body, offset, context);
        }
        Statement::For(for_stmt) => {
            scoped_symbol_context_in_expr(&for_stmt.iter, offset, context);
            scoped_symbol_context_in_statements(&for_stmt.body, offset, context);
        }
        Statement::Unsafe(unsafe_stmt) => {
            scoped_symbol_context_in_statements(&unsafe_stmt.body, offset, context);
        }
        Statement::Expr(expr) => scoped_symbol_context_in_expr(expr, offset, context),
        Statement::Assert(assert_stmt) => {
            match &assert_stmt.kind {
                incan_frontend::ast::AssertKind::Condition(expr) => {
                    scoped_symbol_context_in_expr(expr, offset, context);
                }
                incan_frontend::ast::AssertKind::IsPattern { value, .. } => {
                    scoped_symbol_context_in_expr(value, offset, context);
                }
                incan_frontend::ast::AssertKind::Raises { call, .. } => {
                    scoped_symbol_context_in_expr(call, offset, context);
                }
            }
            if let Some(message) = &assert_stmt.message {
                scoped_symbol_context_in_expr(message, offset, context);
            }
        }
        Statement::CompoundAssignment(assign) => scoped_symbol_context_in_expr(&assign.value, offset, context),
        Statement::TupleUnpack(assign) => scoped_symbol_context_in_expr(&assign.value, offset, context),
        Statement::ChainedAssignment(assign) => scoped_symbol_context_in_expr(&assign.value, offset, context),
        Statement::VocabExpressionItem(item) => {
            scoped_symbol_context_in_expr(&item.expr, offset, context);
            for modifier in &item.modifiers {
                scoped_symbol_context_in_expr(&modifier.value, offset, context);
            }
        }
        Statement::TupleAssign(assign) => {
            for target in &assign.targets {
                scoped_symbol_context_in_expr(target, offset, context);
            }
            scoped_symbol_context_in_expr(&assign.value, offset, context);
        }
        Statement::VocabBlock(block) => {
            let previous_len = context.vocab_stack.len();
            context.vocab_stack.push(block.keyword.clone());
            scoped_symbol_context_in_statements(&block.body, offset, context);
            let matched_body = block
                .body
                .iter()
                .any(|stmt| stmt.span.start <= offset && offset <= stmt.span.end);
            if !matched_body {
                context.vocab_stack.truncate(previous_len);
            }
        }
        Statement::Pass | Statement::Continue | Statement::Surface(_) => {}
    }
}

/// Update scoped symbol completion context from a control-flow condition containing the offset.
pub(super) fn scoped_symbol_context_in_condition(
    condition: &Condition,
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    match condition {
        Condition::Expr(expr) => scoped_symbol_context_in_expr(expr, offset, context),
        Condition::Let { value, .. } => scoped_symbol_context_in_expr(value, offset, context),
    }
}

/// Update scoped symbol completion context from one expression containing the offset.
pub(super) fn scoped_symbol_context_in_expr(expr: &Spanned<Expr>, offset: usize, context: &mut ScopedSymbolLspContext) {
    if !(expr.span.start <= offset && offset <= expr.span.end) {
        return;
    }
    match &expr.node {
        Expr::Call(callee, _, args) => {
            scoped_symbol_context_in_expr(callee, offset, context);
            if offset >= callee.span.end && offset <= expr.span.end {
                context.call_target = call_argument_target_lsp(callee);
                scoped_symbol_context_in_call_args(args, offset, context);
            }
        }
        Expr::MethodCall(receiver, method, _, args) => {
            scoped_symbol_context_in_expr(receiver, offset, context);
            if offset >= receiver.span.end && offset <= expr.span.end {
                context.call_target = Some(method.clone());
                scoped_symbol_context_in_call_args(args, offset, context);
            }
        }
        Expr::Binary(left, _, right) => {
            scoped_symbol_context_in_expr(left, offset, context);
            scoped_symbol_context_in_expr(right, offset, context);
        }
        Expr::Unary(_, inner)
        | Expr::Index(inner, _)
        | Expr::Slice(inner, _)
        | Expr::Try(inner)
        | Expr::Paren(inner)
        | Expr::Yield(Some(inner)) => scoped_symbol_context_in_expr(inner, offset, context),
        Expr::Match(scrutinee, arms) => {
            scoped_symbol_context_in_expr(scrutinee, offset, context);
            scoped_symbol_context_in_match_arms(arms, offset, context);
        }
        Expr::If(if_expr) => {
            scoped_symbol_context_in_expr(&if_expr.condition, offset, context);
            scoped_symbol_context_in_statements(&if_expr.then_body, offset, context);
            if let Some(body) = &if_expr.else_body {
                scoped_symbol_context_in_statements(body, offset, context);
            }
        }
        Expr::Loop(loop_expr) => scoped_symbol_context_in_statements(&loop_expr.body, offset, context),
        Expr::ListComp(comp) => {
            scoped_symbol_context_in_expr(&comp.expr, offset, context);
            scoped_symbol_context_in_expr(&comp.iter, offset, context);
            if let Some(filter) = &comp.filter {
                scoped_symbol_context_in_expr(filter, offset, context);
            }
            scoped_symbol_context_in_comprehension_clauses(&comp.clauses, offset, context);
        }
        Expr::DictComp(comp) => {
            scoped_symbol_context_in_expr(&comp.key, offset, context);
            scoped_symbol_context_in_expr(&comp.value, offset, context);
            scoped_symbol_context_in_expr(&comp.iter, offset, context);
            if let Some(filter) = &comp.filter {
                scoped_symbol_context_in_expr(filter, offset, context);
            }
            scoped_symbol_context_in_comprehension_clauses(&comp.clauses, offset, context);
        }
        Expr::Generator(generator) => {
            scoped_symbol_context_in_expr(&generator.expr, offset, context);
            scoped_symbol_context_in_comprehension_clauses(&generator.clauses, offset, context);
        }
        Expr::Closure(_, body) => scoped_symbol_context_in_expr(body, offset, context),
        Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                scoped_symbol_context_in_expr(item, offset, context);
            }
        }
        Expr::List(entries) => {
            for entry in entries {
                match entry {
                    ListEntry::Element(expr) | ListEntry::Spread(expr) => {
                        scoped_symbol_context_in_expr(expr, offset, context);
                    }
                }
            }
        }
        Expr::Dict(entries) => scoped_symbol_context_in_dict_entries(entries, offset, context),
        Expr::Partial(partial) => {
            scoped_symbol_context_in_expr(&partial.target, offset, context);
            for arg in &partial.args {
                scoped_symbol_context_in_expr(&arg.value, offset, context);
            }
        }
        Expr::Constructor(_, args) => scoped_symbol_context_in_call_args(args, offset, context),
        Expr::FString(parts) => {
            for part in parts {
                if let incan_frontend::ast::FStringPart::Expr { expr, .. } = part {
                    scoped_symbol_context_in_expr(expr, offset, context);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            scoped_symbol_context_in_expr(start, offset, context);
            scoped_symbol_context_in_expr(end, offset, context);
        }
        Expr::Surface(surface) => scoped_symbol_context_in_surface(&surface.payload, offset, context),
        Expr::VocabBlock(block) => scoped_symbol_context_in_vocab_block(block, offset, context),
        Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr | Expr::Yield(None) => {}
        Expr::Field(inner, _) => scoped_symbol_context_in_expr(inner, offset, context),
        // Completion context follows the cursor into a fragment's expression holes, which are ordinary Incan
        // (RFC 081); the surrounding DSL syntax carries no scoped-symbol context of its own.
        Expr::Embedded(fragment) => {
            for hole in fragment.holes() {
                scoped_symbol_context_in_expr(hole, offset, context);
            }
        }
    }
}

/// Update match-arm context in guard/body order after its scrutinee. Each recursive call retains its own cursor guard.
pub(super) fn scoped_symbol_context_in_match_arms(
    arms: &[Spanned<incan_frontend::ast::MatchArm>],
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    for arm in arms {
        if let Some(guard) = &arm.node.guard {
            scoped_symbol_context_in_expr(guard, offset, context);
        }
        match &arm.node.body {
            MatchBody::Expr(expr) => scoped_symbol_context_in_expr(expr, offset, context),
            MatchBody::Block(body) => scoped_symbol_context_in_statements(body, offset, context),
        }
    }
}

/// Visit dictionary keys before their values and spread operands in entry order using the caller's live context.
pub(super) fn scoped_symbol_context_in_dict_entries(
    entries: &[DictEntry],
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    for entry in entries {
        match entry {
            DictEntry::Pair(key, value) => {
                scoped_symbol_context_in_expr(key, offset, context);
                scoped_symbol_context_in_expr(value, offset, context);
            }
            DictEntry::Spread(expr) => scoped_symbol_context_in_expr(expr, offset, context),
        }
    }
}

/// Follow scoped surface expressions in their original order, including race awaitables before each body. Leading-dot
/// paths contribute no nested expression context; the caller retains vocabulary stack restoration.
pub(super) fn scoped_symbol_context_in_surface(
    payload: &SurfaceExprPayload,
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    match payload {
        SurfaceExprPayload::PrefixUnary(inner) => scoped_symbol_context_in_expr(inner, offset, context),
        SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
            scoped_symbol_context_in_expr(left, offset, context);
            scoped_symbol_context_in_expr(right, offset, context);
        }
        SurfaceExprPayload::ScopedSymbolCall { args, .. } => scoped_symbol_context_in_call_args(args, offset, context),
        SurfaceExprPayload::RaceFor(race) => {
            for arm in &race.arms {
                scoped_symbol_context_in_expr(&arm.awaitable, offset, context);
                match &arm.body {
                    RaceForBody::Expr(expr) => scoped_symbol_context_in_expr(expr, offset, context),
                    RaceForBody::Block(statements) => scoped_symbol_context_in_statements(statements, offset, context),
                }
            }
        }
        SurfaceExprPayload::LeadingDotPath { .. } => {}
    }
}

/// Retain a vocabulary frame only when the cursor matches a body statement. Header-only positions restore the
/// previous stack depth after the same header/body traversal, preserving nested vocabulary completion scope.
pub(super) fn scoped_symbol_context_in_vocab_block(
    block: &incan_frontend::ast::VocabBlockStmt,
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    let previous_len = context.vocab_stack.len();
    context.vocab_stack.push(block.keyword.clone());
    for arg in &block.header_args {
        scoped_symbol_context_in_expr(arg, offset, context);
    }
    scoped_symbol_context_in_statements(&block.body, offset, context);
    let matched_body = block
        .body
        .iter()
        .any(|stmt| stmt.span.start <= offset && offset <= stmt.span.end);
    if !matched_body {
        context.vocab_stack.truncate(previous_len);
    }
}

/// Update scoped symbol completion context from comprehension clauses containing the offset.
pub(super) fn scoped_symbol_context_in_comprehension_clauses(
    clauses: &[incan_frontend::ast::ComprehensionClause],
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    for clause in clauses {
        match clause {
            incan_frontend::ast::ComprehensionClause::For { iter, .. } => {
                scoped_symbol_context_in_expr(iter, offset, context);
            }
            incan_frontend::ast::ComprehensionClause::If(condition) => {
                scoped_symbol_context_in_expr(condition, offset, context);
            }
        }
    }
}

/// Update scoped symbol completion context from call arguments containing the offset.
pub(super) fn scoped_symbol_context_in_call_args(
    args: &[CallArg],
    offset: usize,
    context: &mut ScopedSymbolLspContext,
) {
    for arg in args {
        match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => scoped_symbol_context_in_expr(expr, offset, context),
        }
    }
}

/// Return the identifier-like callee name used by call-argument scoped symbol eligibility.
pub(super) fn call_argument_target_lsp(expr: &Spanned<Expr>) -> Option<String> {
    match &expr.node {
        Expr::Ident(name) | Expr::Field(_, name) => Some(name.clone()),
        _ => None,
    }
}
