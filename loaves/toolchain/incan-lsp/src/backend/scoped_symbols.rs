//! DSL-scoped symbol hovers, embedded-fragment ownership, and scoped symbol lookup at an offset.

use super::*;

/// Build hover markdown for a DSL-scoped symbol descriptor.
pub(super) fn scoped_symbol_hover_markdown(
    dependency_key: &str,
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> String {
    let mut markdown = format!(
        "```incan\n{}(...)\n```\n\n*scoped DSL symbol* from `pub::{dependency_key}`",
        descriptor.symbol
    );
    markdown.push_str(&format!("\n\nDescriptor: `{}`", descriptor.key));
    markdown.push_str(&format!(
        "\n\nFamily: `{}`",
        scoped_symbol_family_label(descriptor.family)
    ));
    if let Some(role) = &descriptor.role {
        if let Some(label) = role.label.as_deref().filter(|label| !label.is_empty()) {
            markdown.push_str(&format!("\n\nRole: `{}` ({label})", role.key));
        } else {
            markdown.push_str(&format!("\n\nRole: `{}`", role.key));
        }
    }
    markdown
}

/// Find the embedded fragment whose *DSL-owned* syntax covers `offset`, if any.
///
/// Ordinary resolution in this server is identifier-driven and source-text-driven: `identifier_at_offset` scans
/// raw bytes with no idea whether they belong to Incan. Inside `<section class="card">` the bytes `section` and
/// `class` are perfectly good ASCII identifiers, so without this check hover would look them up in the document's
/// value and symbol maps and answer with whatever unrelated Incan binding happens to share the spelling. RFC 081
/// requires the opposite: tooling must make ownership visible enough that a reader can tell where ordinary Incan
/// ends and the DSL-owned surface begins.
///
/// Traversal reuses the canonical [`ast_walk::any_expr_in_program`] rather than adding another walk, and the
/// hole-versus-DSL decision is [`EmbeddedFragmentExpr::ownership_at`], which is itself built on
/// [`EmbeddedFragmentExpr::holes`] — the single authority on which expressions a fragment owns. Because the walk
/// descends through holes, a fragment nested inside another fragment's hole wins over its container, which is the
/// innermost-descriptor rule RFC 081 states for overlapping claims.
///
/// [`EmbeddedFragmentExpr::holes`]: incan_frontend::ast::EmbeddedFragmentExpr::holes
/// [`EmbeddedFragmentExpr::ownership_at`]: incan_frontend::ast::EmbeddedFragmentExpr::ownership_at
pub(super) fn embedded_dsl_ownership_at_offset(ast: &Program, offset: usize) -> Option<EmbeddedDslOwnership> {
    let mut found: Option<EmbeddedDslOwnership> = None;
    ast_walk::any_expr_in_program(ast, |expr| {
        let Expr::Embedded(fragment) = expr else {
            return false;
        };
        if !matches!(fragment.ownership_at(offset), Some(EmbeddedOwnership::DslOwned)) {
            return false;
        }
        let Some(span) = fragment.node_extent() else {
            return false;
        };
        let (dependency_key, descriptor_key) = match &fragment.key {
            incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                dependency_key,
                descriptor_key,
            } => (dependency_key.clone(), descriptor_key.clone()),
            _ => (String::new(), String::new()),
        };
        found = Some(EmbeddedDslOwnership {
            submode: fragment.submode,
            dependency_key,
            descriptor_key,
            span,
        });
        true
    });
    found
}

/// Return a stable, human-readable label for an embedded-fragment submode.
///
/// These are the names RFC 081 and the vocab authoring guide use for the six accepted grammars, so a reader who
/// hovers a fragment and then goes looking for what it accepts finds the same word in the docs. The enum is
/// `#[non_exhaustive]`, so a submode added later falls back to `unknown` rather than failing to build here — the
/// same fallback `scoped_symbol_family_label` uses.
pub(super) fn embedded_submode_label(submode: incan_vocab::EmbeddedFragmentSubmode) -> &'static str {
    match submode {
        incan_vocab::EmbeddedFragmentSubmode::Markup => "Markup",
        incan_vocab::EmbeddedFragmentSubmode::Style => "Style",
        incan_vocab::EmbeddedFragmentSubmode::RawText => "RawText",
        incan_vocab::EmbeddedFragmentSubmode::RegexTemplate => "RegexTemplate",
        incan_vocab::EmbeddedFragmentSubmode::SelectorDeclarationValue => "SelectorDeclarationValue",
        incan_vocab::EmbeddedFragmentSubmode::TypePosition => "TypePosition",
        _ => "unknown",
    }
}

/// Build the hover markdown shown over an embedded fragment's DSL-owned syntax.
///
/// The message answers the two questions a reader actually has at that position: who owns this text, and why does
/// nothing here resolve like ordinary Incan. It deliberately does not describe the construct under the cursor as a
/// tag, selector, or property — RFC 081 assigns that meaning to the owning DSL, and inventing a name for it here
/// would be the compiler guessing at a grammar it does not own.
pub(super) fn embedded_dsl_ownership_hover_markdown(ownership: &EmbeddedDslOwnership) -> String {
    let mut markdown = format!(
        "```incan\n{} fragment\n```\n\n*embedded fragment* — DSL-owned syntax, not ordinary Incan.",
        embedded_submode_label(ownership.submode)
    );
    if !ownership.descriptor_key.is_empty() {
        markdown.push_str(&format!("\n\nDescriptor: `{}`", ownership.descriptor_key));
    }
    if !ownership.dependency_key.is_empty() {
        markdown.push_str(&format!("\n\nFrom: `pub::{}`", ownership.dependency_key));
    }
    markdown.push_str(
        "\n\nNames written here resolve against the owning DSL, not against Incan scope. Only `{...}` expression \
         holes inside this fragment are ordinary Incan (RFC 081).",
    );
    markdown
}

/// Return a stable, human-readable label for a scoped symbol family.
pub(super) fn scoped_symbol_family_label(family: incan_vocab::ScopedSymbolFamily) -> &'static str {
    match family {
        incan_vocab::ScopedSymbolFamily::FunctionLike => "function-like",
        incan_vocab::ScopedSymbolFamily::AggregateLike => "aggregate-like",
        incan_vocab::ScopedSymbolFamily::PredicateLike => "predicate-like",
        incan_vocab::ScopedSymbolFamily::ProjectionLike => "projection-like",
        incan_vocab::ScopedSymbolFamily::GroupingLike => "grouping-like",
        incan_vocab::ScopedSymbolFamily::OrderingLike => "ordering-like",
        incan_vocab::ScopedSymbolFamily::WindowLike => "window-like",
        _ => "unknown",
    }
}

/// Build completion detail text for a DSL-scoped symbol descriptor.
pub(super) fn scoped_symbol_completion_detail(
    dependency_key: &str,
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> String {
    let family = scoped_symbol_family_label(descriptor.family);
    if let Some(role) = &descriptor.role {
        if let Some(label) = role.label.as_deref().filter(|label| !label.is_empty()) {
            return format!("scoped DSL {family} from pub::{dependency_key} ({label})");
        }
        return format!("scoped DSL {family} from pub::{dependency_key} ({})", role.key);
    }
    format!("scoped DSL {family} from pub::{dependency_key}")
}

/// Find the scoped symbol occurrence under `offset`, if the parsed AST accepted it as a DSL symbol call.
pub(super) fn scoped_symbol_at_offset<'a>(
    ast: &'a Program,
    source: &str,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    offset: usize,
) -> Option<ScopedSymbolOccurrence<'a>> {
    let (ident, symbol_span) = identifier_at_offset(source, offset)?;
    let mut found = None;
    for decl in &ast.declarations {
        scoped_symbol_in_declaration(decl, &ident, symbol_span, surfaces, &mut found);
        if found.is_some() {
            break;
        }
    }
    found
}

/// Search one declaration for a parsed scoped symbol occurrence.
pub(super) fn scoped_symbol_in_declaration<'a>(
    decl: &'a Spanned<Declaration>,
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    if found.is_some() || !(decl.span.start <= symbol_span.start && symbol_span.end <= decl.span.end) {
        return;
    }
    match &decl.node {
        Declaration::Const(konst) => scoped_symbol_in_expr(&konst.value, ident, symbol_span, surfaces, found),
        Declaration::Static(static_decl) => {
            scoped_symbol_in_expr(&static_decl.value, ident, symbol_span, surfaces, found);
        }
        Declaration::Function(func) => scoped_symbol_in_statements(&func.body, ident, symbol_span, surfaces, found),
        Declaration::Model(model) => {
            for method in &model.methods {
                if let Some(body) = &method.node.body {
                    scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
                }
            }
            for property in &model.properties {
                if let Some(body) = &property.node.body {
                    scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
                }
            }
        }
        Declaration::Class(class) => {
            for method in &class.methods {
                if let Some(body) = &method.node.body {
                    scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
                }
            }
            for property in &class.properties {
                if let Some(body) = &property.node.body {
                    scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
                }
            }
        }
        Declaration::Trait(trait_decl) => {
            for method in &trait_decl.methods {
                if let Some(body) = &method.node.body {
                    scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
                }
            }
            for property in &trait_decl.properties {
                if let Some(body) = &property.node.body {
                    scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
                }
            }
        }
        _ => {}
    }
}

/// Search a statement list for a parsed scoped symbol occurrence.
pub(super) fn scoped_symbol_in_statements<'a>(
    statements: &'a [Spanned<Statement>],
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for stmt in statements {
        if found.is_some() {
            break;
        }
        scoped_symbol_in_statement(stmt, ident, symbol_span, surfaces, found);
    }
}

/// Search one statement for a parsed scoped symbol occurrence.
pub(super) fn scoped_symbol_in_statement<'a>(
    stmt: &'a Spanned<Statement>,
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    if found.is_some() || !(stmt.span.start <= symbol_span.start && symbol_span.end <= stmt.span.end) {
        return;
    }
    match &stmt.node {
        Statement::Assignment(assign) => scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found),
        Statement::FieldAssignment(assign) => {
            scoped_symbol_in_expr(&assign.object, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found);
        }
        Statement::IndexAssignment(assign) => {
            scoped_symbol_in_expr(&assign.object, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(&assign.index, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found);
        }
        Statement::Return(expr) | Statement::Break(expr) => {
            if let Some(expr) = expr {
                scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found);
            }
        }
        Statement::If(if_stmt) => {
            scoped_symbol_in_condition(&if_stmt.condition, ident, symbol_span, surfaces, found);
            scoped_symbol_in_statements(&if_stmt.then_body, ident, symbol_span, surfaces, found);
            for (condition, body) in &if_stmt.elif_branches {
                scoped_symbol_in_expr(condition, ident, symbol_span, surfaces, found);
                scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
            }
            if let Some(body) = &if_stmt.else_body {
                scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
            }
        }
        Statement::Loop(loop_stmt) => scoped_symbol_in_statements(&loop_stmt.body, ident, symbol_span, surfaces, found),
        Statement::While(while_stmt) => {
            scoped_symbol_in_condition(&while_stmt.condition, ident, symbol_span, surfaces, found);
            scoped_symbol_in_statements(&while_stmt.body, ident, symbol_span, surfaces, found);
        }
        Statement::For(for_stmt) => {
            scoped_symbol_in_expr(&for_stmt.iter, ident, symbol_span, surfaces, found);
            scoped_symbol_in_statements(&for_stmt.body, ident, symbol_span, surfaces, found);
        }
        Statement::Unsafe(unsafe_stmt) => {
            scoped_symbol_in_statements(&unsafe_stmt.body, ident, symbol_span, surfaces, found);
        }
        Statement::Expr(expr) => scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found),
        Statement::Assert(assert_stmt) => {
            match &assert_stmt.kind {
                incan_frontend::ast::AssertKind::Condition(expr) => {
                    scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found);
                }
                incan_frontend::ast::AssertKind::IsPattern { value, .. } => {
                    scoped_symbol_in_expr(value, ident, symbol_span, surfaces, found);
                }
                incan_frontend::ast::AssertKind::Raises { call, .. } => {
                    scoped_symbol_in_expr(call, ident, symbol_span, surfaces, found);
                }
            }
            if let Some(message) = &assert_stmt.message {
                scoped_symbol_in_expr(message, ident, symbol_span, surfaces, found);
            }
        }
        Statement::CompoundAssignment(assign) => {
            scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found);
        }
        Statement::TupleUnpack(assign) => scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found),
        Statement::ChainedAssignment(assign) => {
            scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found);
        }
        Statement::VocabExpressionItem(item) => {
            scoped_symbol_in_expr(&item.expr, ident, symbol_span, surfaces, found);
            for modifier in &item.modifiers {
                scoped_symbol_in_expr(&modifier.value, ident, symbol_span, surfaces, found);
            }
        }
        Statement::TupleAssign(assign) => {
            for target in &assign.targets {
                scoped_symbol_in_expr(target, ident, symbol_span, surfaces, found);
            }
            scoped_symbol_in_expr(&assign.value, ident, symbol_span, surfaces, found);
        }
        Statement::VocabBlock(block) => {
            scoped_symbol_in_statements(&block.body, ident, symbol_span, surfaces, found);
        }
        Statement::Pass | Statement::Continue | Statement::Surface(_) => {}
    }
}

/// Search a control-flow condition for a parsed scoped symbol occurrence.
pub(super) fn scoped_symbol_in_condition<'a>(
    condition: &'a Condition,
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    match condition {
        Condition::Expr(expr) => scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found),
        Condition::Let { value, .. } => scoped_symbol_in_expr(value, ident, symbol_span, surfaces, found),
    }
}

/// Search one expression tree for a parsed scoped symbol occurrence.
pub(super) fn scoped_symbol_in_expr<'a>(
    expr: &'a Spanned<Expr>,
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    if found.is_some() || !(expr.span.start <= symbol_span.start && symbol_span.end <= expr.span.end) {
        return;
    }
    if let Expr::Surface(surface) = &expr.node
        && let incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
            dependency_key,
            descriptor_key,
        } = &surface.key
        && let SurfaceExprPayload::ScopedSymbolCall { symbol, .. } = &surface.payload
        && symbol == ident
        && symbol_span.start == expr.span.start
        && let Some(descriptor) = scoped_symbol_descriptor(surfaces, dependency_key, descriptor_key)
    {
        *found = Some(ScopedSymbolOccurrence {
            dependency_key,
            descriptor,
            symbol_span,
        });
        return;
    }

    match &expr.node {
        Expr::Binary(left, _, right) => {
            scoped_symbol_in_expr(left, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(right, ident, symbol_span, surfaces, found);
        }
        Expr::Unary(_, inner)
        | Expr::Index(inner, _)
        | Expr::Slice(inner, _)
        | Expr::Try(inner)
        | Expr::Paren(inner)
        | Expr::Yield(Some(inner)) => scoped_symbol_in_expr(inner, ident, symbol_span, surfaces, found),
        Expr::Call(callee, _, args) => {
            scoped_symbol_in_expr(callee, ident, symbol_span, surfaces, found);
            scoped_symbol_in_call_args(args, ident, symbol_span, surfaces, found);
        }
        Expr::MethodCall(receiver, _, _, args) => {
            scoped_symbol_in_expr(receiver, ident, symbol_span, surfaces, found);
            scoped_symbol_in_call_args(args, ident, symbol_span, surfaces, found);
        }
        Expr::Match(scrutinee, arms) => {
            scoped_symbol_in_expr(scrutinee, ident, symbol_span, surfaces, found);
            scoped_symbol_in_match_arms(arms, ident, symbol_span, surfaces, found);
        }
        Expr::If(if_expr) => {
            scoped_symbol_in_expr(&if_expr.condition, ident, symbol_span, surfaces, found);
            scoped_symbol_in_statements(&if_expr.then_body, ident, symbol_span, surfaces, found);
            if let Some(body) = &if_expr.else_body {
                scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found);
            }
        }
        Expr::Loop(loop_expr) => scoped_symbol_in_statements(&loop_expr.body, ident, symbol_span, surfaces, found),
        Expr::ListComp(comp) => {
            scoped_symbol_in_expr(&comp.expr, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(&comp.iter, ident, symbol_span, surfaces, found);
            if let Some(filter) = &comp.filter {
                scoped_symbol_in_expr(filter, ident, symbol_span, surfaces, found);
            }
            scoped_symbol_in_comprehension_clauses(&comp.clauses, ident, symbol_span, surfaces, found);
        }
        Expr::DictComp(comp) => {
            scoped_symbol_in_expr(&comp.key, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(&comp.value, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(&comp.iter, ident, symbol_span, surfaces, found);
            if let Some(filter) = &comp.filter {
                scoped_symbol_in_expr(filter, ident, symbol_span, surfaces, found);
            }
            scoped_symbol_in_comprehension_clauses(&comp.clauses, ident, symbol_span, surfaces, found);
        }
        Expr::Generator(generator) => {
            scoped_symbol_in_expr(&generator.expr, ident, symbol_span, surfaces, found);
            scoped_symbol_in_comprehension_clauses(&generator.clauses, ident, symbol_span, surfaces, found);
        }
        Expr::Closure(_, body) => scoped_symbol_in_expr(body, ident, symbol_span, surfaces, found),
        Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                scoped_symbol_in_expr(item, ident, symbol_span, surfaces, found);
            }
        }
        Expr::List(entries) => scoped_symbol_in_list_entries(entries, ident, symbol_span, surfaces, found),
        Expr::Dict(entries) => scoped_symbol_in_dict_entries(entries, ident, symbol_span, surfaces, found),
        Expr::Partial(partial) => {
            scoped_symbol_in_expr(&partial.target, ident, symbol_span, surfaces, found);
            for arg in &partial.args {
                scoped_symbol_in_expr(&arg.value, ident, symbol_span, surfaces, found);
            }
        }
        Expr::Constructor(_, args) => scoped_symbol_in_call_args(args, ident, symbol_span, surfaces, found),
        Expr::FString(parts) => {
            for part in parts {
                if let incan_frontend::ast::FStringPart::Expr { expr, .. } = part {
                    scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            scoped_symbol_in_expr(start, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(end, ident, symbol_span, surfaces, found);
        }
        Expr::Surface(surface) => scoped_symbol_in_surface(&surface.payload, ident, symbol_span, surfaces, found),
        Expr::VocabBlock(block) => scoped_symbol_in_vocab_block(block, ident, symbol_span, surfaces, found),
        Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr | Expr::Yield(None) => {}
        Expr::Field(inner, _) => scoped_symbol_in_expr(inner, ident, symbol_span, surfaces, found),
        // A fragment's expression holes are ordinary Incan (RFC 081), so a scoped symbol written inside one
        // resolves through the same traversal as anywhere else.
        Expr::Embedded(fragment) => {
            for hole in fragment.holes() {
                scoped_symbol_in_expr(hole, ident, symbol_span, surfaces, found);
            }
        }
    }
}

/// Search match guards before their bodies after the caller has searched the scrutinee. The shared first-hit slot
/// keeps later recursive visits from replacing an already resolved source occurrence.
pub(super) fn scoped_symbol_in_match_arms<'a>(
    arms: &'a [Spanned<incan_frontend::ast::MatchArm>],
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for arm in arms {
        if let Some(guard) = &arm.node.guard {
            scoped_symbol_in_expr(guard, ident, symbol_span, surfaces, found);
        }
        match &arm.node.body {
            MatchBody::Expr(expr) => scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found),
            MatchBody::Block(body) => scoped_symbol_in_statements(body, ident, symbol_span, surfaces, found),
        }
    }
}

/// Search list elements and spreads in entry order without copying their source syntax or surface descriptors.
pub(super) fn scoped_symbol_in_list_entries<'a>(
    entries: &'a [ListEntry],
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for entry in entries {
        match entry {
            ListEntry::Element(expr) | ListEntry::Spread(expr) => {
                scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found)
            }
        }
    }
}

/// Search dictionary keys before values, then spread operands, using the same first-hit source occurrence slot.
pub(super) fn scoped_symbol_in_dict_entries<'a>(
    entries: &'a [DictEntry],
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for entry in entries {
        match entry {
            DictEntry::Pair(key, value) => {
                scoped_symbol_in_expr(key, ident, symbol_span, surfaces, found);
                scoped_symbol_in_expr(value, ident, symbol_span, surfaces, found);
            }
            DictEntry::Spread(expr) => scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found),
        }
    }
}

/// Visit only a surface payload's ordinary expressions. Race awaitables precede bodies, scoped glyphs visit left
/// before right, and leading-dot paths contribute no nested occurrence.
pub(super) fn scoped_symbol_in_surface<'a>(
    payload: &'a SurfaceExprPayload,
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    match payload {
        SurfaceExprPayload::PrefixUnary(inner) => scoped_symbol_in_expr(inner, ident, symbol_span, surfaces, found),
        SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
            scoped_symbol_in_expr(left, ident, symbol_span, surfaces, found);
            scoped_symbol_in_expr(right, ident, symbol_span, surfaces, found);
        }
        SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
            scoped_symbol_in_call_args(args, ident, symbol_span, surfaces, found)
        }
        SurfaceExprPayload::RaceFor(race) => {
            for arm in &race.arms {
                scoped_symbol_in_expr(&arm.awaitable, ident, symbol_span, surfaces, found);
                match &arm.body {
                    RaceForBody::Expr(expr) => scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found),
                    RaceForBody::Block(statements) => {
                        scoped_symbol_in_statements(statements, ident, symbol_span, surfaces, found)
                    }
                }
            }
        }
        SurfaceExprPayload::LeadingDotPath { .. } => {}
    }
}

/// Search vocabulary header arguments before body statements, retaining source-borrowed occurrence metadata.
pub(super) fn scoped_symbol_in_vocab_block<'a>(
    block: &'a incan_frontend::ast::VocabBlockStmt,
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for arg in &block.header_args {
        scoped_symbol_in_expr(arg, ident, symbol_span, surfaces, found);
    }
    scoped_symbol_in_statements(&block.body, ident, symbol_span, surfaces, found);
}

/// Search comprehension clauses for parsed scoped symbol occurrences.
pub(super) fn scoped_symbol_in_comprehension_clauses<'a>(
    clauses: &'a [incan_frontend::ast::ComprehensionClause],
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for clause in clauses {
        match clause {
            incan_frontend::ast::ComprehensionClause::For { iter, .. } => {
                scoped_symbol_in_expr(iter, ident, symbol_span, surfaces, found);
            }
            incan_frontend::ast::ComprehensionClause::If(condition) => {
                scoped_symbol_in_expr(condition, ident, symbol_span, surfaces, found);
            }
        }
    }
}

/// Search call arguments for parsed scoped symbol occurrences.
pub(super) fn scoped_symbol_in_call_args<'a>(
    args: &'a [CallArg],
    ident: &str,
    symbol_span: Span,
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    found: &mut Option<ScopedSymbolOccurrence<'a>>,
) {
    for arg in args {
        match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => scoped_symbol_in_expr(expr, ident, symbol_span, surfaces, found),
        }
    }
}

/// Resolve a scoped symbol descriptor by dependency key and descriptor key.
pub(super) fn scoped_symbol_descriptor<'a>(
    surfaces: &'a parser::ImportedLibraryDslSurfaces,
    dependency_key: &str,
    descriptor_key: &str,
) -> Option<&'a incan_vocab::ScopedSymbolDescriptor> {
    surfaces
        .get(dependency_key)?
        .iter()
        .flat_map(|surface| surface.scoped_symbols.iter())
        .find(|descriptor| descriptor.key == descriptor_key)
}

/// Find the nearest activating `pub::` import span before a scoped symbol use.
pub(super) fn find_pub_library_import_span(ast: &Program, dependency_key: &str, before_offset: usize) -> Option<Span> {
    ast.declarations.iter().rev().find_map(|decl| {
        if decl.span.start >= before_offset {
            return None;
        }
        let Declaration::Import(import) = &decl.node else {
            return None;
        };
        match &import.kind {
            incan_frontend::ast::ImportKind::PubLibrary { library, .. }
            | incan_frontend::ast::ImportKind::PubFrom { library, .. }
                if library == dependency_key =>
            {
                Some(decl.span)
            }
            _ => None,
        }
    })
}
