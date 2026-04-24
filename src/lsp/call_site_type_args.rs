//! Call-site explicit generic arguments (`callee[T](...)`, `recv.m[U](...)`) — LSP helpers (RFC 054).

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::frontend::ast::{
    CallArg, Condition, Declaration, Expr, MatchBody, Program, SliceExpr, Spanned, Statement, Type,
};
use incan_core::lang::conventions;
use incan_core::lang::types::{collections, numerics, stringlike};

// ---- Bracket scan (works with nested generics, e.g. `f[Dict[str, int]](...)`) ----

fn find_enclosing_open_bracket(bytes: &[u8], scan_before: usize) -> Option<usize> {
    let mut i = scan_before.checked_sub(1)?;
    let mut depth = 0i32;
    loop {
        match bytes.get(i).copied() {
            Some(b']') => depth += 1,
            Some(b'[') => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            Some(_) => {}
            None => return None,
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

fn find_matching_close_bracket(bytes: &[u8], open_idx: usize) -> Option<usize> {
    let mut i = open_idx + 1;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Outermost `[`…`]` around `offset` whose closing `]` is immediately followed by `(` (call-site type list).
fn outer_call_site_type_brackets(source: &str, offset: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    // Search only strictly to the left of this byte index (so we can skip a rejected `[` pair).
    let mut scan_before = offset.min(bytes.len());
    loop {
        let open = find_enclosing_open_bracket(bytes, scan_before)?;
        let close = find_matching_close_bracket(bytes, open)?;
        let tail = source.get(close + 1..).unwrap_or("").trim_start();
        if tail.starts_with('(') {
            return Some((open, close));
        }
        if open == 0 {
            return None;
        }
        scan_before = open;
    }
}

/// `true` when `offset` is strictly inside the `[` … `]` of a call-site type argument list.
pub(crate) fn offset_in_call_site_type_argument_list(source: &str, offset: usize) -> bool {
    let Some((open, close)) = outer_call_site_type_brackets(source, offset) else {
        return false;
    };
    open < offset && offset < close
}

// ---- Innermost [`Spanned<Type>`] under cursor inside a call ----

pub(crate) fn innermost_type_node_at_offset(ty: &Spanned<Type>, offset: usize) -> Option<&Spanned<Type>> {
    if offset < ty.span.start || offset >= ty.span.end {
        return None;
    }
    match &ty.node {
        Type::Generic(_, args) | Type::Tuple(args) => {
            for a in args {
                if let Some(hit) = innermost_type_node_at_offset(a, offset) {
                    return Some(hit);
                }
            }
            Some(ty)
        }
        Type::Function(params, ret) => {
            for p in params {
                if let Some(hit) = innermost_type_node_at_offset(p, offset) {
                    return Some(hit);
                }
            }
            if let Some(hit) = innermost_call_site_type_in_type(ret, offset) {
                return Some(hit);
            }
            Some(ty)
        }
        Type::Qualified(_) | Type::Simple(_) | Type::Unit | Type::SelfType | Type::Infer => Some(ty),
    }
}

fn innermost_call_site_type_in_type(ty: &Spanned<Type>, offset: usize) -> Option<&Spanned<Type>> {
    innermost_type_node_at_offset(ty, offset)
}

fn scan_types_in_call(type_args: &[Spanned<Type>], offset: usize) -> Option<&Spanned<Type>> {
    for t in type_args {
        if let Some(hit) = innermost_type_node_at_offset(t, offset) {
            return Some(hit);
        }
    }
    None
}

fn scan_call_args(args: &[CallArg], offset: usize) -> Option<&Spanned<Type>> {
    for a in args {
        match a {
            CallArg::Positional(e) => {
                if let Some(hit) = call_site_type_in_expr(e, offset) {
                    return Some(hit);
                }
            }
            CallArg::Named(_, e) => {
                if let Some(hit) = call_site_type_in_expr(e, offset) {
                    return Some(hit);
                }
            }
        }
    }
    None
}

fn call_site_type_in_expr(expr: &Spanned<Expr>, offset: usize) -> Option<&Spanned<Type>> {
    match &expr.node {
        Expr::Call(callee, type_args, args) => {
            if let Some(hit) = scan_types_in_call(type_args, offset) {
                return Some(hit);
            }
            if let Some(hit) = call_site_type_in_expr(callee, offset) {
                return Some(hit);
            }
            scan_call_args(args, offset)
        }
        Expr::MethodCall(base, _, type_args, args) => {
            if let Some(hit) = scan_types_in_call(type_args, offset) {
                return Some(hit);
            }
            if let Some(hit) = call_site_type_in_expr(base, offset) {
                return Some(hit);
            }
            scan_call_args(args, offset)
        }
        Expr::Binary(a, _, b) => call_site_type_in_expr(a, offset).or_else(|| call_site_type_in_expr(b, offset)),
        Expr::Unary(_, a) => call_site_type_in_expr(a, offset),
        Expr::Index(a, i) => call_site_type_in_expr(a, offset).or_else(|| call_site_type_in_expr(i, offset)),
        Expr::Slice(a, s) => call_site_type_in_expr(a, offset).or_else(|| slice_expr_call_site_types(s, offset)),
        Expr::Field(a, _) => call_site_type_in_expr(a, offset),
        Expr::Try(a) => call_site_type_in_expr(a, offset),
        Expr::Match(scr, arms) => {
            if let Some(hit) = call_site_type_in_expr(scr, offset) {
                return Some(hit);
            }
            for arm in arms {
                if let Some(g) = &arm.node.guard
                    && let Some(hit) = call_site_type_in_expr(g, offset)
                {
                    return Some(hit);
                }
                match &arm.node.body {
                    MatchBody::Expr(e) => {
                        if let Some(hit) = call_site_type_in_expr(e, offset) {
                            return Some(hit);
                        }
                    }
                    MatchBody::Block(stmts) => {
                        if let Some(hit) = call_site_types_in_stmts(stmts, offset) {
                            return Some(hit);
                        }
                    }
                }
            }
            None
        }
        Expr::If(boxed) => {
            if let Some(hit) = call_site_type_in_expr(&boxed.condition, offset) {
                return Some(hit);
            }
            if let Some(hit) = call_site_types_in_stmts(&boxed.then_body, offset) {
                return Some(hit);
            }
            if let Some(else_body) = &boxed.else_body {
                return call_site_types_in_stmts(else_body, offset);
            }
            None
        }
        Expr::ListComp(boxed) => call_site_type_in_expr(&boxed.expr, offset)
            .or_else(|| call_site_type_in_expr(&boxed.iter, offset))
            .or_else(|| boxed.filter.as_ref().and_then(|e| call_site_type_in_expr(e, offset))),
        Expr::DictComp(boxed) => call_site_type_in_expr(&boxed.key, offset)
            .or_else(|| call_site_type_in_expr(&boxed.value, offset))
            .or_else(|| call_site_type_in_expr(&boxed.iter, offset))
            .or_else(|| boxed.filter.as_ref().and_then(|e| call_site_type_in_expr(e, offset))),
        Expr::Closure(_, body) => call_site_type_in_expr(body, offset),
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => {
            items.iter().find_map(|e| call_site_type_in_expr(e, offset))
        }
        Expr::Dict(pairs) => pairs
            .iter()
            .find_map(|(k, v)| call_site_type_in_expr(k, offset).or_else(|| call_site_type_in_expr(v, offset))),
        Expr::Paren(inner) => call_site_type_in_expr(inner, offset),
        Expr::Constructor(_, args) => scan_call_args(args, offset),
        Expr::FString(parts) => parts.iter().find_map(|p| {
            if let crate::frontend::ast::FStringPart::Expr(e) = p {
                call_site_type_in_expr(e, offset)
            } else {
                None
            }
        }),
        Expr::Yield(Some(inner)) => call_site_type_in_expr(inner, offset),
        Expr::Yield(None) => None,
        Expr::Range { start, end, .. } => {
            call_site_type_in_expr(start, offset).or_else(|| call_site_type_in_expr(end, offset))
        }
        Expr::Surface(boxed) => match &boxed.payload {
            crate::frontend::ast::SurfaceExprPayload::PrefixUnary(inner) => call_site_type_in_expr(inner, offset),
        },
        Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr => None,
    }
}

fn slice_expr_call_site_types(s: &SliceExpr, offset: usize) -> Option<&Spanned<Type>> {
    s.start
        .as_ref()
        .and_then(|b| call_site_type_in_expr(b, offset))
        .or_else(|| s.end.as_ref().and_then(|b| call_site_type_in_expr(b, offset)))
        .or_else(|| s.step.as_ref().and_then(|b| call_site_type_in_expr(b, offset)))
}

fn call_site_types_in_stmts(stmts: &[Spanned<Statement>], offset: usize) -> Option<&Spanned<Type>> {
    for s in stmts {
        if let Some(hit) = call_site_type_in_stmt(&s.node, offset) {
            return Some(hit);
        }
    }
    None
}

fn call_site_type_in_stmt(stmt: &Statement, offset: usize) -> Option<&Spanned<Type>> {
    match stmt {
        Statement::Assignment(a) => call_site_type_in_expr(&a.value, offset),
        Statement::FieldAssignment(f) => {
            call_site_type_in_expr(&f.object, offset).or_else(|| call_site_type_in_expr(&f.value, offset))
        }
        Statement::IndexAssignment(i) => call_site_type_in_expr(&i.object, offset)
            .or_else(|| call_site_type_in_expr(&i.index, offset))
            .or_else(|| call_site_type_in_expr(&i.value, offset)),
        Statement::Return(Some(e)) => call_site_type_in_expr(e, offset),
        Statement::Return(None) => None,
        Statement::If(i) => call_site_type_in_condition(&i.condition, offset)
            .or_else(|| call_site_types_in_stmts(&i.then_body, offset))
            .or_else(|| {
                i.elif_branches.iter().find_map(|(c, b)| {
                    call_site_type_in_expr(c, offset).or_else(|| call_site_types_in_stmts(b, offset))
                })
            })
            .or_else(|| i.else_body.as_ref().and_then(|b| call_site_types_in_stmts(b, offset))),
        Statement::While(w) => {
            call_site_type_in_condition(&w.condition, offset).or_else(|| call_site_types_in_stmts(&w.body, offset))
        }
        Statement::For(f) => {
            call_site_type_in_expr(&f.iter, offset).or_else(|| call_site_types_in_stmts(&f.body, offset))
        }
        Statement::Expr(e) => call_site_type_in_expr(e, offset),
        Statement::CompoundAssignment(c) => call_site_type_in_expr(&c.value, offset),
        Statement::TupleUnpack(t) => call_site_type_in_expr(&t.value, offset),
        Statement::TupleAssign(t) => t
            .targets
            .iter()
            .find_map(|e| call_site_type_in_expr(e, offset))
            .or_else(|| call_site_type_in_expr(&t.value, offset)),
        Statement::ChainedAssignment(c) => call_site_type_in_expr(&c.value, offset),
        Statement::Surface(s) => match &s.payload {
            crate::frontend::ast::SurfaceStmtPayload::KeywordArgs(exprs) => {
                exprs.iter().find_map(|e| call_site_type_in_expr(e, offset))
            }
        },
        Statement::Pass | Statement::Break | Statement::Continue => None,
        Statement::VocabBlock(_) => None,
    }
}

/// Search a control-flow condition for explicit call-site type arguments at the
/// requested offset.
///
/// Let-pattern conditions only expose type arguments from the scrutinee
/// expression; pattern nodes themselves do not currently carry call-site type
/// argument syntax.
fn call_site_type_in_condition(condition: &Condition, offset: usize) -> Option<&Spanned<Type>> {
    match condition {
        Condition::Expr(expr) => call_site_type_in_expr(expr, offset),
        Condition::Let { value, .. } => call_site_type_in_expr(value, offset),
    }
}

fn call_site_type_in_declaration(decl: &Declaration, offset: usize) -> Option<&Spanned<Type>> {
    match decl {
        Declaration::Function(f) => call_site_types_in_stmts(&f.body, offset),
        Declaration::Const(c) => call_site_type_in_expr(&c.value, offset),
        Declaration::Static(s) => call_site_type_in_expr(&s.value, offset),
        Declaration::Model(m) => {
            for field in &m.fields {
                if let Some(def) = &field.node.default
                    && let Some(hit) = call_site_type_in_expr(def, offset)
                {
                    return Some(hit);
                }
            }
            for meth in &m.methods {
                if let Some(body) = &meth.node.body
                    && let Some(hit) = call_site_types_in_stmts(body, offset)
                {
                    return Some(hit);
                }
            }
            None
        }
        Declaration::Class(c) => {
            for field in &c.fields {
                if let Some(def) = &field.node.default
                    && let Some(hit) = call_site_type_in_expr(def, offset)
                {
                    return Some(hit);
                }
            }
            for meth in &c.methods {
                if let Some(body) = &meth.node.body
                    && let Some(hit) = call_site_types_in_stmts(body, offset)
                {
                    return Some(hit);
                }
            }
            None
        }
        Declaration::Trait(t) => {
            for meth in &t.methods {
                if let Some(body) = &meth.node.body
                    && let Some(hit) = call_site_types_in_stmts(body, offset)
                {
                    return Some(hit);
                }
            }
            None
        }
        Declaration::Newtype(n) => {
            for rb in &n.rebindings {
                if let Some(hit) = call_site_type_in_expr(&rb.node.target, offset) {
                    return Some(hit);
                }
            }
            for edge in &n.interop_edges {
                if let Some(hit) = call_site_type_in_expr(&edge.node.adapter, offset) {
                    return Some(hit);
                }
            }
            for meth in &n.methods {
                if let Some(body) = &meth.node.body
                    && let Some(hit) = call_site_types_in_stmts(body, offset)
                {
                    return Some(hit);
                }
            }
            None
        }
        _ => None,
    }
}

/// Deepest [`Type`] AST node inside an explicit call-site `[...]` that contains `offset`.
pub(crate) fn call_site_innermost_type_at_offset(program: &Program, offset: usize) -> Option<&Spanned<Type>> {
    for decl in &program.declarations {
        if let Some(hit) = call_site_type_in_declaration(&decl.node, offset) {
            return Some(hit);
        }
    }
    None
}

// ---- Completions (type-oriented list) ----

fn push(items: &mut Vec<CompletionItem>, seen: &mut std::collections::HashSet<String>, item: CompletionItem) {
    if seen.insert(item.label.clone()) {
        items.push(item);
    }
}

/// Completions appropriate inside `callee[|](...)`: `_`, builtins, collections, and local type declarations.
pub(crate) fn call_site_type_argument_completion_items(ast: Option<&Program>) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    push(
        &mut items,
        &mut seen,
        CompletionItem {
            label: "_".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("Infer this type parameter from value arguments (RFC 054)".to_string()),
            sort_text: Some("0__".to_string()),
            ..Default::default()
        },
    );

    push(
        &mut items,
        &mut seen,
        CompletionItem {
            label: "Self".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("Receiver type in trait/impl method context".to_string()),
            sort_text: Some("0_Self".to_string()),
            ..Default::default()
        },
    );

    for t in numerics::NUMERIC_TYPES {
        push(
            &mut items,
            &mut seen,
            CompletionItem {
                label: t.canonical.to_string(),
                kind: Some(CompletionItemKind::TYPE_PARAMETER),
                detail: Some(t.description.to_string()),
                sort_text: Some(format!("1_{}", t.canonical)),
                ..Default::default()
            },
        );
        for a in t.aliases {
            push(
                &mut items,
                &mut seen,
                CompletionItem {
                    label: (*a).to_string(),
                    kind: Some(CompletionItemKind::TYPE_PARAMETER),
                    detail: Some(format!("alias of {}", t.canonical)),
                    sort_text: Some(format!("2_{}", a)),
                    ..Default::default()
                },
            );
        }
    }

    for t in stringlike::STRING_LIKE_TYPES {
        push(
            &mut items,
            &mut seen,
            CompletionItem {
                label: t.canonical.to_string(),
                kind: Some(CompletionItemKind::TYPE_PARAMETER),
                detail: Some(t.description.to_string()),
                sort_text: Some(format!("1_{}", t.canonical)),
                ..Default::default()
            },
        );
        for a in t.aliases {
            push(
                &mut items,
                &mut seen,
                CompletionItem {
                    label: (*a).to_string(),
                    kind: Some(CompletionItemKind::TYPE_PARAMETER),
                    detail: Some(format!("alias of {}", t.canonical)),
                    sort_text: Some(format!("2_{}", a)),
                    ..Default::default()
                },
            );
        }
    }

    for info in collections::COLLECTION_TYPES {
        push(
            &mut items,
            &mut seen,
            CompletionItem {
                label: info.canonical.to_string(),
                kind: Some(CompletionItemKind::CLASS),
                detail: Some(info.description.to_string()),
                sort_text: Some(format!("1_{}", info.canonical)),
                ..Default::default()
            },
        );
        for a in info.aliases {
            push(
                &mut items,
                &mut seen,
                CompletionItem {
                    label: (*a).to_string(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some(format!("alias of {}", info.canonical)),
                    sort_text: Some(format!("2_{}", a)),
                    ..Default::default()
                },
            );
        }
    }

    for name in [conventions::UNIT_TYPE_NAME, conventions::NONE_TYPE_NAME] {
        push(
            &mut items,
            &mut seen,
            CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::TYPE_PARAMETER),
                detail: Some("Builtin type".to_string()),
                sort_text: Some(format!("1_{}", name)),
                ..Default::default()
            },
        );
    }

    if let Some(program) = ast {
        for decl in &program.declarations {
            match &decl.node {
                Declaration::Model(m) => push(
                    &mut items,
                    &mut seen,
                    CompletionItem {
                        label: m.name.clone(),
                        kind: Some(CompletionItemKind::STRUCT),
                        detail: Some(format!("model {}", m.name)),
                        sort_text: Some(format!("0_{}", m.name)),
                        ..Default::default()
                    },
                ),
                Declaration::Class(c) => push(
                    &mut items,
                    &mut seen,
                    CompletionItem {
                        label: c.name.clone(),
                        kind: Some(CompletionItemKind::CLASS),
                        detail: Some(format!("class {}", c.name)),
                        sort_text: Some(format!("0_{}", c.name)),
                        ..Default::default()
                    },
                ),
                Declaration::Trait(t) => push(
                    &mut items,
                    &mut seen,
                    CompletionItem {
                        label: t.name.clone(),
                        kind: Some(CompletionItemKind::INTERFACE),
                        detail: Some(format!("trait {}", t.name)),
                        sort_text: Some(format!("0_{}", t.name)),
                        ..Default::default()
                    },
                ),
                Declaration::Enum(e) => push(
                    &mut items,
                    &mut seen,
                    CompletionItem {
                        label: e.name.clone(),
                        kind: Some(CompletionItemKind::ENUM),
                        detail: Some(format!("enum {}", e.name)),
                        sort_text: Some(format!("0_{}", e.name)),
                        ..Default::default()
                    },
                ),
                Declaration::TypeAlias(a) => push(
                    &mut items,
                    &mut seen,
                    CompletionItem {
                        label: a.name.clone(),
                        kind: Some(CompletionItemKind::TYPE_PARAMETER),
                        detail: Some(format!("type alias {}", a.name)),
                        sort_text: Some(format!("0_{}", a.name)),
                        ..Default::default()
                    },
                ),
                Declaration::Newtype(n) => {
                    let kind = if n.is_rusttype { "rusttype" } else { "newtype" };
                    push(
                        &mut items,
                        &mut seen,
                        CompletionItem {
                            label: n.name.clone(),
                            kind: Some(CompletionItemKind::STRUCT),
                            detail: Some(format!("{kind} {}", n.name)),
                            sort_text: Some(format!("0_{}", n.name)),
                            ..Default::default()
                        },
                    );
                }
                _ => {}
            }
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_call_site_brackets_with_nested_generics() -> Result<(), &'static str> {
        let s = "f[Dict[str, int]](x)";
        // Byte offset inside `int` in nested generics (avoids unwrap on `find`)
        let off = 13;
        if !offset_in_call_site_type_argument_list(s, off) {
            return Err("expected offset inside call-site type argument list");
        }
        let (o, c) = outer_call_site_type_brackets(s, off).ok_or("expected outer call-site type brackets")?;
        assert_eq!(&s[o..=c], "[Dict[str, int]]");
        Ok(())
    }

    #[test]
    fn index_expression_is_not_call_site_type_list() {
        let s = "arr[0]";
        assert!(!offset_in_call_site_type_argument_list(s, 4));
    }

    #[test]
    fn incomplete_call_site_without_paren_after_bracket_is_not_inside_type_list() {
        let s = "id[int]";
        // Byte offset inside `int`; no `(` after `]` so this is not a call-site type-arg list
        let off = 4;
        assert!(!offset_in_call_site_type_argument_list(s, off));
    }
}
