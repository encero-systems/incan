//! Source signature formatting and local call signature help.

use super::*;

/// Format a function signature for display using source-backed default expressions.
pub(super) fn format_function_signature(func: &incan_frontend::ast::FunctionDecl, source: &str) -> String {
    let mut sig = String::new();

    if func.is_async() {
        sig.push_str("async ");
    }

    sig.push_str("def ");
    sig.push_str(&func.name);
    sig.push('(');

    let params: Vec<String> = func
        .params
        .iter()
        .map(|param| format_local_param(param, source))
        .collect();

    sig.push_str(&params.join(", "));
    sig.push(')');

    sig.push_str(" -> ");
    sig.push_str(&format_type(&func.return_type.node));
    if let Some(source) = &func.result_source {
        sig.push_str(" from ");
        sig.push_str(&source.node.binding.node);
        for field in &source.node.fields {
            sig.push('.');
            sig.push_str(&field.node);
        }
    }

    sig
}

/// Format a module-level partial declaration for hover, completion detail, and document symbols.
pub(super) fn format_partial_decl_signature(partial: &incan_frontend::ast::PartialDecl, source: &str) -> String {
    let args = partial
        .args
        .iter()
        .map(|arg| {
            format!(
                "{}={}",
                arg.name,
                source_snippet(source, arg.value.span).unwrap_or("...")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} = partial {}({})",
        partial.name,
        partial.target.segments.join("."),
        args
    )
}

/// Format one AST parameter with the same local default-expression display used by hover, completion, and signature
/// help.
pub(super) fn format_local_param(param: &Spanned<Param>, source: &str) -> String {
    let prefix = match param.node.kind {
        ParamKind::Normal => "",
        ParamKind::RestPositional => "*",
        ParamKind::RestKeyword => "**",
    };
    let mutability = if param.node.is_mut { "mut " } else { "" };
    let default = param
        .node
        .default
        .as_ref()
        .map(|default| format!(" = {}", source_snippet(source, default.span).unwrap_or("...")))
        .unwrap_or_default();
    format!(
        "{prefix}{mutability}{}: {}{default}",
        param.node.name,
        format_type(&param.node.ty.node)
    )
}

/// Return a trimmed source slice for a known parser span.
pub(super) fn source_snippet(source: &str, span: Span) -> Option<&str> {
    source
        .get(span.start..span.end)
        .map(str::trim)
        .filter(|snippet| !snippet.is_empty())
}

/// Build LSP signature help for a parsed local function call at the cursor offset.
pub(super) fn local_signature_help_at_offset(ast: &Program, source: &str, offset: usize) -> Option<SignatureHelp> {
    let signature = local_call_signature_at_offset(ast, source, offset)?;
    let parameters = signature
        .parameters
        .into_iter()
        .map(|label| ParameterInformation {
            label: ParameterLabel::Simple(label),
            documentation: None,
        })
        .collect();
    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: signature.label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: signature.active_parameter,
        }],
        active_signature: Some(0),
        active_parameter: signature.active_parameter,
    })
}

/// Return the local function signature associated with the innermost call containing an offset.
pub(super) fn local_call_signature_at_offset(ast: &Program, source: &str, offset: usize) -> Option<LocalCallSignature> {
    for declaration in &ast.declarations {
        let hit = match &declaration.node {
            Declaration::Function(function) => local_signature_in_statements(&function.body, ast, source, offset),
            Declaration::Model(model) => local_signature_in_methods(&model.methods, ast, source, offset),
            Declaration::Class(class) => local_signature_in_methods(&class.methods, ast, source, offset),
            Declaration::Trait(trait_decl) => local_signature_in_methods(&trait_decl.methods, ast, source, offset),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// Search method bodies for local function calls that can provide signature help.
pub(super) fn local_signature_in_methods(
    methods: &[Spanned<MethodDecl>],
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    methods
        .iter()
        .filter(|method| method.span.start <= offset && offset <= method.span.end)
        .filter_map(|method| method.node.body.as_ref())
        .find_map(|body| local_signature_in_statements(body, ast, source, offset))
}

/// Search a statement list for the innermost local function call containing an offset.
pub(super) fn local_signature_in_statements(
    statements: &[Spanned<Statement>],
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    statements
        .iter()
        .filter(|statement| statement.span.start <= offset && offset <= statement.span.end)
        .find_map(|statement| local_signature_in_statement(&statement.node, ast, source, offset))
}

/// Search one statement for local function calls that can provide signature help.
pub(super) fn local_signature_in_statement(
    statement: &Statement,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    match statement {
        Statement::Assignment(assignment) => local_signature_in_expr(&assignment.value, ast, source, offset),
        Statement::FieldAssignment(assignment) => local_signature_in_expr(&assignment.object, ast, source, offset)
            .or_else(|| local_signature_in_expr(&assignment.value, ast, source, offset)),
        Statement::IndexAssignment(assignment) => local_signature_in_expr(&assignment.object, ast, source, offset)
            .or_else(|| local_signature_in_expr(&assignment.index, ast, source, offset))
            .or_else(|| local_signature_in_expr(&assignment.value, ast, source, offset)),
        Statement::Return(Some(value)) => local_signature_in_expr(value, ast, source, offset),
        Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => None,
        Statement::If(if_stmt) => local_signature_in_condition(&if_stmt.condition, ast, source, offset)
            .or_else(|| local_signature_in_statements(&if_stmt.then_body, ast, source, offset))
            .or_else(|| {
                if_stmt.elif_branches.iter().find_map(|(condition, body)| {
                    local_signature_in_expr(condition, ast, source, offset)
                        .or_else(|| local_signature_in_statements(body, ast, source, offset))
                })
            })
            .or_else(|| {
                if_stmt
                    .else_body
                    .as_ref()
                    .and_then(|body| local_signature_in_statements(body, ast, source, offset))
            }),
        Statement::Loop(loop_stmt) => local_signature_in_statements(&loop_stmt.body, ast, source, offset),
        Statement::While(while_stmt) => local_signature_in_condition(&while_stmt.condition, ast, source, offset)
            .or_else(|| local_signature_in_statements(&while_stmt.body, ast, source, offset)),
        Statement::For(for_stmt) => local_signature_in_expr(&for_stmt.iter, ast, source, offset)
            .or_else(|| local_signature_in_statements(&for_stmt.body, ast, source, offset)),
        Statement::Unsafe(unsafe_stmt) => local_signature_in_statements(&unsafe_stmt.body, ast, source, offset),
        Statement::Expr(expr) => local_signature_in_expr(expr, ast, source, offset),
        Statement::Assert(assert_stmt) => local_signature_in_assert(assert_stmt, ast, source, offset),
        Statement::Break(Some(value)) => local_signature_in_expr(value, ast, source, offset),
        Statement::CompoundAssignment(assignment) => local_signature_in_expr(&assignment.value, ast, source, offset),
        Statement::TupleUnpack(unpack) => local_signature_in_expr(&unpack.value, ast, source, offset),
        Statement::TupleAssign(assign) => assign
            .targets
            .iter()
            .find_map(|target| local_signature_in_expr(target, ast, source, offset))
            .or_else(|| local_signature_in_expr(&assign.value, ast, source, offset)),
        Statement::ChainedAssignment(assignment) => local_signature_in_expr(&assignment.value, ast, source, offset),
        Statement::VocabExpressionItem(item) => {
            local_signature_in_expr(&item.expr, ast, source, offset).or_else(|| {
                item.modifiers
                    .iter()
                    .find_map(|modifier| local_signature_in_expr(&modifier.value, ast, source, offset))
            })
        }
        Statement::Surface(surface) => match &surface.payload {
            incan_frontend::ast::SurfaceStmtPayload::KeywordArgs(args) => args
                .iter()
                .find_map(|arg| local_signature_in_expr(arg, ast, source, offset)),
        },
        Statement::VocabBlock(block) => block
            .header_args
            .iter()
            .find_map(|arg| local_signature_in_expr(arg, ast, source, offset))
            .or_else(|| local_signature_in_statements(&block.body, ast, source, offset)),
    }
}

/// Search a condition expression for local function calls that can provide signature help.
pub(super) fn local_signature_in_condition(
    condition: &Condition,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    match condition {
        Condition::Expr(expr) => local_signature_in_expr(expr, ast, source, offset),
        Condition::Let { value, .. } => local_signature_in_expr(value, ast, source, offset),
    }
}

/// Search an assertion payload for local function calls that can provide signature help.
pub(super) fn local_signature_in_assert(
    assert_stmt: &incan_frontend::ast::AssertStmt,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    let payload = match &assert_stmt.kind {
        incan_frontend::ast::AssertKind::Condition(condition) => {
            local_signature_in_expr(condition, ast, source, offset)
        }
        incan_frontend::ast::AssertKind::IsPattern { value, .. } => local_signature_in_expr(value, ast, source, offset),
        incan_frontend::ast::AssertKind::Raises { call, .. } => local_signature_in_expr(call, ast, source, offset),
    };
    payload.or_else(|| {
        assert_stmt
            .message
            .as_ref()
            .and_then(|message| local_signature_in_expr(message, ast, source, offset))
    })
}

/// Search one expression for the innermost local function call containing an offset.
pub(super) fn local_signature_in_expr(
    expr: &Spanned<Expr>,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    if offset < expr.span.start || expr.span.end < offset {
        return None;
    }
    match &expr.node {
        Expr::Call(callee, _type_args, args) => local_signature_in_expr(callee, ast, source, offset)
            .or_else(|| local_signature_in_call_args(args, ast, source, offset))
            .or_else(|| local_function_call_signature(expr, callee, ast, source, offset)),
        Expr::MethodCall(receiver, _method, _type_args, args) => local_signature_in_expr(receiver, ast, source, offset)
            .or_else(|| local_signature_in_call_args(args, ast, source, offset)),
        Expr::Binary(left, _, right) => local_signature_in_expr(left, ast, source, offset)
            .or_else(|| local_signature_in_expr(right, ast, source, offset)),
        Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) => {
            local_signature_in_expr(inner, ast, source, offset)
        }
        Expr::Index(base, index) => local_signature_in_expr(base, ast, source, offset)
            .or_else(|| local_signature_in_expr(index, ast, source, offset)),
        Expr::Slice(base, slice) => local_signature_in_expr(base, ast, source, offset)
            .or_else(|| local_signature_in_slice(slice, ast, source, offset)),
        Expr::Field(base, _) => local_signature_in_expr(base, ast, source, offset),
        Expr::Match(scrutinee, arms) => local_signature_in_expr(scrutinee, ast, source, offset)
            .or_else(|| local_signature_in_match_arms(arms, ast, source, offset)),
        Expr::If(if_expr) => local_signature_in_expr(&if_expr.condition, ast, source, offset)
            .or_else(|| local_signature_in_statements(&if_expr.then_body, ast, source, offset))
            .or_else(|| {
                if_expr
                    .else_body
                    .as_ref()
                    .and_then(|body| local_signature_in_statements(body, ast, source, offset))
            }),
        Expr::Loop(loop_expr) => local_signature_in_statements(&loop_expr.body, ast, source, offset),
        Expr::ListComp(list_comp) => local_signature_in_expr(&list_comp.expr, ast, source, offset)
            .or_else(|| local_signature_in_expr(&list_comp.iter, ast, source, offset))
            .or_else(|| {
                list_comp
                    .filter
                    .as_ref()
                    .and_then(|filter| local_signature_in_expr(filter, ast, source, offset))
            }),
        Expr::DictComp(dict_comp) => local_signature_in_expr(&dict_comp.key, ast, source, offset)
            .or_else(|| local_signature_in_expr(&dict_comp.value, ast, source, offset))
            .or_else(|| local_signature_in_expr(&dict_comp.iter, ast, source, offset))
            .or_else(|| {
                dict_comp
                    .filter
                    .as_ref()
                    .and_then(|filter| local_signature_in_expr(filter, ast, source, offset))
            }),
        Expr::Generator(generator) => local_signature_in_expr(&generator.expr, ast, source, offset).or_else(|| {
            generator.clauses.iter().find_map(|clause| match clause {
                incan_frontend::ast::ComprehensionClause::For { iter, .. } => {
                    local_signature_in_expr(iter, ast, source, offset)
                }
                incan_frontend::ast::ComprehensionClause::If(condition) => {
                    local_signature_in_expr(condition, ast, source, offset)
                }
            })
        }),
        Expr::Closure(params, body) => params
            .iter()
            .filter_map(|param| param.node.default.as_ref())
            .find_map(|default| local_signature_in_expr(default, ast, source, offset))
            .or_else(|| local_signature_in_expr(body, ast, source, offset)),
        Expr::Tuple(items) | Expr::Set(items) => items
            .iter()
            .find_map(|item| local_signature_in_expr(item, ast, source, offset)),
        Expr::List(items) => items.iter().find_map(|item| match item {
            incan_frontend::ast::ListEntry::Element(value) | incan_frontend::ast::ListEntry::Spread(value) => {
                local_signature_in_expr(value, ast, source, offset)
            }
        }),
        Expr::Dict(items) => items.iter().find_map(|item| match item {
            DictEntry::Pair(key, value) => local_signature_in_expr(key, ast, source, offset)
                .or_else(|| local_signature_in_expr(value, ast, source, offset)),
            DictEntry::Spread(value) => local_signature_in_expr(value, ast, source, offset),
        }),
        Expr::Partial(partial) => local_signature_in_expr(&partial.target, ast, source, offset).or_else(|| {
            partial
                .args
                .iter()
                .find_map(|arg| local_signature_in_expr(&arg.value, ast, source, offset))
        }),
        Expr::Constructor(_, args) => local_signature_in_call_args(args, ast, source, offset),
        Expr::FString(parts) => parts.iter().find_map(|part| match part {
            incan_frontend::ast::FStringPart::Expr { expr, .. } => local_signature_in_expr(expr, ast, source, offset),
            incan_frontend::ast::FStringPart::Literal(_) => None,
        }),
        Expr::Yield(Some(value)) => local_signature_in_expr(value, ast, source, offset),
        Expr::Yield(None) => None,
        Expr::Range { start, end, .. } => local_signature_in_expr(start, ast, source, offset)
            .or_else(|| local_signature_in_expr(end, ast, source, offset)),
        Expr::Surface(surface) => local_signature_in_surface(&surface.payload, ast, source, offset),
        Expr::VocabBlock(block) => block
            .header_args
            .iter()
            .find_map(|arg| local_signature_in_expr(arg, ast, source, offset))
            .or_else(|| local_signature_in_statements(&block.body, ast, source, offset)),
        // An embedded fragment's expression holes are ordinary Incan (RFC 081), so signature help has to reach them
        // through the same traversal the typechecker uses. Treating the fragment as opaque made a call written
        // inside a hole lose signature help even though nothing about that call is embedded.
        Expr::Embedded(fragment) => fragment
            .holes()
            .into_iter()
            .find_map(|hole| local_signature_in_expr(hole, ast, source, offset)),
        Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr => None,
    }
}

/// Search optional slice bounds in start/end/step order after the caller has searched the base expression.
pub(super) fn local_signature_in_slice(
    slice: &incan_frontend::ast::SliceExpr,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    slice
        .start
        .as_ref()
        .and_then(|start| local_signature_in_expr(start, ast, source, offset))
        .or_else(|| {
            slice
                .end
                .as_ref()
                .and_then(|end| local_signature_in_expr(end, ast, source, offset))
        })
        .or_else(|| {
            slice
                .step
                .as_ref()
                .and_then(|step| local_signature_in_expr(step, ast, source, offset))
        })
}

/// Search match guards before their expression or statement bodies, retaining the first signature in arm order.
pub(super) fn local_signature_in_match_arms(
    arms: &[Spanned<incan_frontend::ast::MatchArm>],
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    arms.iter().find_map(|arm| {
        arm.node
            .guard
            .as_ref()
            .and_then(|guard| local_signature_in_expr(guard, ast, source, offset))
            .or_else(|| match &arm.node.body {
                MatchBody::Expr(expr) => local_signature_in_expr(expr, ast, source, offset),
                MatchBody::Block(statements) => local_signature_in_statements(statements, ast, source, offset),
            })
    })
}

/// Follow source surface payloads without introducing another signature authority. Race awaitables precede their
/// bodies; leading-dot paths contain no ordinary expression to search.
pub(super) fn local_signature_in_surface(
    payload: &SurfaceExprPayload,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    match payload {
        SurfaceExprPayload::PrefixUnary(inner) => local_signature_in_expr(inner, ast, source, offset),
        SurfaceExprPayload::RaceFor(race) => race.arms.iter().find_map(|arm| {
            local_signature_in_expr(&arm.awaitable, ast, source, offset).or_else(|| match &arm.body {
                RaceForBody::Expr(expr) => local_signature_in_expr(expr, ast, source, offset),
                RaceForBody::Block(statements) => local_signature_in_statements(statements, ast, source, offset),
            })
        }),
        SurfaceExprPayload::LeadingDotPath { .. } => None,
        SurfaceExprPayload::ScopedGlyph { left, right, .. } => local_signature_in_expr(left, ast, source, offset)
            .or_else(|| local_signature_in_expr(right, ast, source, offset)),
        SurfaceExprPayload::ScopedSymbolCall { args, .. } => local_signature_in_call_args(args, ast, source, offset),
    }
}

/// Search call arguments for nested local function calls.
pub(super) fn local_signature_in_call_args(
    args: &[CallArg],
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    args.iter().find_map(|arg| match arg {
        CallArg::Positional(expr)
        | CallArg::Named(_, expr)
        | CallArg::PositionalUnpack(expr)
        | CallArg::KeywordUnpack(expr) => local_signature_in_expr(expr, ast, source, offset),
    })
}

/// Build a local function signature for a direct `name(...)` call when the cursor is inside its argument list.
pub(super) fn local_function_call_signature(
    call_expr: &Spanned<Expr>,
    callee: &Spanned<Expr>,
    ast: &Program,
    source: &str,
    offset: usize,
) -> Option<LocalCallSignature> {
    let Expr::Ident(name) = &callee.node else {
        return None;
    };
    let (open_paren, close_paren) = call_argument_list_bounds(source, call_expr.span)?;
    if offset <= open_paren || close_paren < offset {
        return None;
    }
    let function = local_function_decl(ast, name)?;
    let parameters = function
        .params
        .iter()
        .map(|param| format_local_param(param, source))
        .collect::<Vec<_>>();
    let active_parameter = active_parameter_index(source, open_paren, offset, parameters.len());
    Some(LocalCallSignature {
        label: format_function_signature(function, source),
        parameters,
        active_parameter,
    })
}

/// Return one top-level function declaration by source name.
pub(super) fn local_function_decl<'a>(ast: &'a Program, name: &str) -> Option<&'a incan_frontend::ast::FunctionDecl> {
    ast.declarations.iter().find_map(|declaration| match &declaration.node {
        Declaration::Function(function) if function.name == name => Some(function),
        _ => None,
    })
}

/// Return the byte offsets of a parsed call expression's argument-list parentheses.
pub(super) fn call_argument_list_bounds(source: &str, span: Span) -> Option<(usize, usize)> {
    let text = source.get(span.start..span.end)?;
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    (open < close).then_some((span.start + open, span.start + close))
}

/// Compute the active parameter index from top-level commas before the cursor.
pub(super) fn active_parameter_index(
    source: &str,
    open_paren: usize,
    offset: usize,
    parameter_count: usize,
) -> Option<u32> {
    if parameter_count == 0 {
        return None;
    }
    let start = open_paren.checked_add(1)?;
    let text = source.get(start..offset.min(source.len()))?;
    let mut nested = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut commas = 0usize;
    for ch in text.chars() {
        if let Some(quote_ch) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote_ch {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '[' | '{' => nested = nested.saturating_add(1),
            ')' | ']' | '}' => nested = nested.saturating_sub(1),
            ',' if nested == 0 => commas = commas.saturating_add(1),
            _ => {}
        }
    }
    Some(commas.min(parameter_count.saturating_sub(1)) as u32)
}

/// Format a computed property declaration for hover and completion details.
pub(super) fn format_property_signature(owner: &str, property: &incan_frontend::ast::PropertyDecl) -> String {
    format!(
        "property {}.{} -> {}",
        owner,
        property.name,
        format_type(&property.return_type.node)
    )
}

/// Return symbol information when an offset falls inside a computed property declaration.
pub(super) fn find_property_symbol_info(
    owner: &str,
    properties: &[incan_frontend::ast::Spanned<incan_frontend::ast::PropertyDecl>],
    offset: usize,
) -> Option<SymbolInfo> {
    for property in properties {
        if property.span.start <= offset && offset < property.span.end {
            return Some(SymbolInfo {
                name: property.node.name.clone(),
                kind: "property".to_string(),
                detail: format_property_signature(owner, &property.node),
                span: property.span,
            });
        }
    }
    None
}

/// Format a Type for display
pub(super) fn format_type(ty: &Type) -> String {
    match ty {
        Type::Simple(name) => name.clone(),
        Type::Qualified(segments) => segments.join("::"),
        Type::Dotted(segments) => segments.join("."),
        Type::Generic(name, params) => {
            let params_str: Vec<String> = params.iter().map(|p| format_type(&p.node)).collect();
            format!("{}[{}]", name, params_str.join(", "))
        }
        Type::DottedGeneric(segments, params) => {
            let params_str: Vec<String> = params.iter().map(|p| format_type(&p.node)).collect();
            format!("{}[{}]", segments.join("."), params_str.join(", "))
        }
        Type::ConstrainedPrimitive(_, _) => ty.to_string(),
        Type::Tuple(types) => {
            let types_str: Vec<String> = types.iter().map(|t| format_type(&t.node)).collect();
            format!("({})", types_str.join(", "))
        }
        Type::Function(params, ret) => {
            let params_str: Vec<String> = params.iter().map(|p| format_type(&p.node)).collect();
            format!("({}) -> {}", params_str.join(", "), format_type(&ret.node))
        }
        Type::Ref(inner) => format!("&{}", format_type(&inner.node)),
        Type::RefMut(inner) => format!("&mut {}", format_type(&inner.node)),
        Type::MutParam(inner) => format!("mut {}", format_type(&inner.node)),
        Type::IntLiteral(value) => value.repr.clone(),
        Type::Unit => "()".to_string(),
        Type::SelfType => "Self".to_string(),
        Type::Infer => "_".to_string(),
    }
}
