//! Translate the published `std.vocab` syntax schema at the compiler boundary, preserving source locations.
//! Only these public shapes cross the worker pipe; compiler AST serialization is never an interchange format.

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ast;

mod declarations;
mod scoped;
use declarations::{SurfaceDeclaration, declarations};

/// Checked worker output ready to enter the ordinary compiler pipeline.
#[derive(Debug, Clone)]
pub(super) enum Output {
    /// Ordinary declarations occupying compilation-unit scope.
    Declarations(Vec<ast::Spanned<ast::Declaration>>),
    /// Statements occupying a registered statement position.
    Statements(Vec<ast::Spanned<ast::Statement>>),
    /// One expression occupying a registered expression position.
    Expression(ast::Spanned<ast::Expr>),
}

/// Decode a public response, rejecting schema drift and out-of-node source locations before AST construction.
pub(super) fn response(value: &Value, anchor: ast::Span, request_identity: &str) -> Result<Output, String> {
    if value.get("schema_version").and_then(Value::as_u64) != Some(u64::from(incan_vocab::VOCAB_SYNTAX_VERSION)) {
        return Err(format!(
            "vocabulary response schema version is unsupported; supported version {}",
            incan_vocab::VOCAB_SYNTAX_VERSION
        ));
    }
    validate_spans(value, anchor)?;
    let mut value = value["output"].clone();
    expand_output_spans(&mut value, anchor, request_identity);
    let output: SurfaceOutput =
        serde_json::from_value(value).map_err(|error| format!("malformed response: {error}"))?;
    match output {
        SurfaceOutput::Declarations(values) => Ok(Output::Declarations(declarations(values, anchor)?)),
        SurfaceOutput::Expression(value) => Ok(Output::Expression(value.into_ast(anchor)?)),
        SurfaceOutput::Statements(values) => Ok(Output::Statements(statements(values, anchor)?)),
    }
}

/// Assign compiler-owned contexts from the vocabulary request and canonical output path before AST decoding.
/// Providers supply only original-source ranges; persistence stores their public response and repeats this derivation.
fn expand_output_spans(value: &mut Value, anchor: ast::Span, path: &str) {
    match value {
        Value::Object(fields) => {
            if fields.contains_key("span") || fields.contains_key("kind") || fields.contains_key("name") {
                let mut span = fields
                    .get("span")
                    .filter(|span| !span.is_null())
                    .cloned()
                    .unwrap_or_else(|| source_span(anchor));
                if let Some(span) = span.as_object_mut() {
                    let identity: [u8; 32] = Sha256::digest(path.as_bytes()).into();
                    span.insert("expansion".into(), json!(identity));
                }
                fields.insert("span".into(), span);
            }
            for (key, child) in fields {
                if key != "span" {
                    expand_output_spans(child, anchor, &format!("{path}/field:{}:{key}", key.len()));
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                expand_output_spans(child, anchor, &format!("{path}/index:{index}"));
            }
        }
        _ => {}
    }
}

/// Ensure every supplied span belongs to the requesting node; absent spans inherit that node's location.
pub(super) fn validate_spans(value: &Value, anchor: ast::Span) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            if let Some(span) = fields.get("span") {
                if span
                    .as_object()
                    .is_some_and(|fields| fields.keys().any(|key| key != "start" && key != "end"))
                {
                    return Err("response source span may contain only original-source byte offsets".into());
                }
                let span: SourceSpan =
                    serde_json::from_value(span.clone()).map_err(|error| format!("malformed source span: {error}"))?;
                if span.start < anchor.start || span.end > anchor.end || span.start > span.end {
                    return Err("response source span lies outside its vocabulary node".into());
                }
            }
            for child in fields.values() {
                validate_spans(child, anchor)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                validate_spans(child, anchor)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Translate a parsed declaration into the versioned public request, excluding compiler routing metadata.
pub(super) fn request(block: &ast::VocabBlockStmt, span: ast::Span, module: &str) -> Result<Value, String> {
    Ok(
        json!({"schema_version": incan_vocab::VOCAB_SYNTAX_VERSION, "module_path": module,
        "node": {"Declaration": declaration(block, span)?}}),
    )
}

/// Preserve declaration heads and clause ownership using parser facts, without re-resolving their meaning.
fn declaration(block: &ast::VocabBlockStmt, span: ast::Span) -> Result<Value, String> {
    let mut args = block.header_args.iter();
    let name = match args.next() {
        Some(ast::Spanned {
            node: ast::Expr::Ident(name),
            ..
        }) => Some(name.clone()),
        _ => {
            args = block.header_args.iter();
            None
        }
    };
    if block.signature_head.is_some() || !block.decorators.is_empty() {
        return Err("vocabulary requests refuse decorators and signature heads; use a declaration with header arguments or no head".into());
    }
    let args = args.map(expression).collect::<Result<Vec<_>, _>>()?;
    let body = block.body.iter().map(body_item).collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"span": source_span(span), "keyword": block.keyword,
        "compound_tokens": block.keyword_binding.compound_tokens,
        "head": {"span": source_span(span), "name": name, "header_args": args,
            "parameters": [], "return_type": null}, "body": body}))
}

/// Classify an authored body item by the parser's retained declaration-owned clause identity.
fn body_item(item: &ast::Spanned<ast::Statement>) -> Result<Value, String> {
    match &item.node {
        ast::Statement::VocabBlock(block) => {
            if block.keyword_binding.is_declaration_owned_clause
                || matches!(
                    block.keyword_binding.surface_kind,
                    incan_vocab::KeywordSurfaceKind::BlockContextKeyword | incan_vocab::KeywordSurfaceKind::SubBlock
                )
            {
                Ok(json!({"Clause": clause(block, item.span)?}))
            } else {
                Ok(json!({"Declaration": declaration(block, item.span)?}))
            }
        }
        _ => Ok(json!({"Statement": statement(item)?})),
    }
}

/// Project the declared clause payload without flattening expression-list aliases and modifiers.
fn clause(block: &ast::VocabBlockStmt, span: ast::Span) -> Result<Value, String> {
    let head = block
        .header_args
        .iter()
        .map(expression)
        .collect::<Result<Vec<_>, _>>()?;
    let body = if block.body.is_empty() {
        json!("Empty")
    } else if matches!(
        block.keyword_binding.clause_body_kind,
        Some(incan_vocab::ClauseBodyKind::ExpressionList)
    ) {
        let items = block.body.iter().map(expression_item).collect::<Result<Vec<_>, _>>()?;
        json!({"ExpressionList": items})
    } else if let [
        ast::Spanned {
            node: ast::Statement::Expr(value),
            ..
        },
    ] = block.body.as_slice()
    {
        json!({"Expression": expression(value)?})
    } else {
        let items = block.body.iter().map(body_item).collect::<Result<Vec<_>, _>>()?;
        json!({"Items": items})
    };
    Ok(json!({"span": source_span(span), "keyword": block.keyword,
        "compound_tokens": block.keyword_binding.compound_tokens, "head": head, "body": body}))
}

/// Preserve each expression-list item's alias and modifier locations.
fn expression_item(item: &ast::Spanned<ast::Statement>) -> Result<Value, String> {
    let (expr, alias, modifiers) = match &item.node {
        ast::Statement::Expr(expr) => (expr, None, Vec::new()),
        ast::Statement::VocabExpressionItem(value) => {
            let modifiers = value
                .modifiers
                .iter()
                .map(|modifier| {
                    Ok(json!({"span": source_span(modifier.span), "keyword": modifier.keyword,
                    "value": expression(&modifier.value)?}))
                })
                .collect::<Result<Vec<_>, String>>()?;
            (&value.expr, value.alias.as_ref(), modifiers)
        }
        _ => return Err("expression-list clause contains a non-expression item".into()),
    };
    Ok(json!({"span": source_span(item.span), "expr": expression(expr)?, "alias": alias, "modifiers": modifiers}))
}

/// Encode one host expression as the public model, retaining an independent span on every child.
pub(super) fn expression(value: &ast::Spanned<ast::Expr>) -> Result<Value, String> {
    let kind = match &value.node {
        ast::Expr::Surface(surface) => json!({"Scoped": scoped::encode(surface)?}),
        ast::Expr::Ident(name) => json!({"Path": [name]}),
        ast::Expr::Literal(ast::Literal::None) => json!("NoneLiteral"),
        ast::Expr::Literal(ast::Literal::Bool(value)) => json!({"Bool": value}),
        ast::Expr::Literal(ast::Literal::Int(value)) if value.suffix.is_none() => json!({"Int": value.value}),
        ast::Expr::Literal(ast::Literal::Float(value)) if value.suffix.is_none() => json!({"Float": value.value}),
        ast::Expr::Literal(ast::Literal::String(value)) => json!({"Str": value}),
        ast::Expr::Embedded(fragment) => json!({"Embedded": super::fragments::fragment(fragment, value.span)?}),
        ast::Expr::Binary(left, op, right) => {
            json!({"Binary": [op.to_string(), [expression(left)?, expression(right)?]]})
        }
        ast::Expr::Unary(op, value) => {
            let op = match op {
                ast::UnaryOp::Neg => "-",
                ast::UnaryOp::Not => "not",
                ast::UnaryOp::Invert => "~",
            };
            json!({"Unary": [op, [expression(value)?]]})
        }
        ast::Expr::Paren(inner) => return expression(inner),
        ast::Expr::Tuple(items) => json!({"Tuple": expressions(items)?}),
        ast::Expr::List(items) => {
            let items = items
                .iter()
                .map(|item| match item {
                    ast::ListEntry::Element(value) => expression(value),
                    _ => Err("public vocabulary schema does not represent list spreads".into()),
                })
                .collect::<Result<Vec<_>, String>>()?;
            json!({"List": items})
        }
        ast::Expr::Dict(items) => {
            let items = items
                .iter()
                .map(|item| match item {
                    ast::DictEntry::Pair(key, value) => Ok(json!([expression(key)?, expression(value)?])),
                    _ => Err("public vocabulary schema does not represent dictionary spreads".into()),
                })
                .collect::<Result<Vec<_>, String>>()?;
            json!({"Dict": items})
        }
        ast::Expr::Call(callee, types, args) if types.is_empty() => {
            let mut call = vec![expression(callee)?];
            call.extend(call_arguments(args)?);
            json!({"Call": call})
        }
        ast::Expr::MethodCall(base, method, types, args) if types.is_empty() => {
            let mut call = vec![json!({"span": source_span(value.span),
                "kind": {"Attribute": [[expression(base)?], method]}})];
            call.extend(call_arguments(args)?);
            json!({"Call": call})
        }
        ast::Expr::Field(base, name) => json!({"Attribute": [[expression(base)?], name]}),
        ast::Expr::Index(base, index) => json!({"Index": [expression(base)?, expression(index)?]}),
        _ => return Err("host expression is not represented by the public vocabulary schema".into()),
    };
    Ok(json!({"span": source_span(value.span), "kind": kind}))
}

/// Encode positional call arguments, refusing forms absent from the published call model.
fn call_arguments(args: &[ast::CallArg]) -> Result<Vec<Value>, String> {
    args.iter()
        .map(|arg| match arg {
            ast::CallArg::Positional(value) => expression(value),
            _ => Err("public vocabulary schema represents positional call arguments only".into()),
        })
        .collect()
}

/// Encode a sequence of independently spanned host expressions.
fn expressions(items: &[ast::Spanned<ast::Expr>]) -> Result<Vec<Value>, String> {
    items.iter().map(expression).collect()
}

/// Encode the statement forms currently exposed by `std.vocab` without serializing compiler statement types.
fn statement(value: &ast::Spanned<ast::Statement>) -> Result<Value, String> {
    let kind = match &value.node {
        ast::Statement::Pass => json!("Pass"),
        ast::Statement::Expr(expr) => json!({"Expr": expression(expr)?}),
        ast::Statement::Return(expr) => json!({"Return": expr.as_ref().map(expression).transpose()?}),
        ast::Statement::Assignment(binding) if binding.ty.is_none() => match binding.binding {
            ast::BindingKind::Reassign => json!({"Assign": [binding.name, expression(&binding.value)?]}),
            kind => json!({"Let": [binding.name, kind == ast::BindingKind::Mutable, expression(&binding.value)?]}),
        },
        _ => return Err("this statement cannot be passed to a vocabulary desugarer; use an expression, return, binding or pass statement".into()),
    };
    Ok(json!({"span": source_span(value.span), "kind": kind}))
}

/// Encode a public half-open source range explicitly, independently of compiler AST layout.
pub(super) fn source_span(span: ast::Span) -> Value {
    json!({"start": span.start, "end": span.end})
}

/// Public source range used only while decoding worker output.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSpan {
    /// Inclusive byte offset.
    start: usize,
    /// Exclusive byte offset.
    end: usize,
    /// Assigned by the compiler after validating the public response, never accepted from a provider.
    expansion: Option<[u8; 32]>,
}

/// Published output-position variants, translated into compiler syntax only after validation.
#[derive(Deserialize)]
enum SurfaceOutput {
    /// Compilation-unit declaration sequence.
    Declarations(Vec<SurfaceDeclaration>),
    /// Host statement sequence.
    Statements(Vec<SurfaceStatement>),
    /// Host expression.
    Expression(SurfaceExpr),
}

/// Binding of the published spanned expression model, with missing spans inheriting the input node.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceExpr {
    /// Original-source range, when explicitly assigned by the provider.
    span: Option<SourceSpan>,
    /// Public expression payload.
    kind: ExprKind,
}

/// Payload alternatives of `std.vocab.IncanExprKind`, kept separate from compiler AST definitions.
#[derive(Deserialize)]
enum ExprKind {
    /// Accepted descriptor-owned syntax is input only; the worker must replace it with ordinary Incan output.
    Scoped(Value),
    /// Owned lexical input; a lowering hook must replace it with ordinary host syntax.
    Embedded(Value),
    /// Unit literal.
    NoneLiteral,
    /// Boolean literal.
    Bool(bool),
    /// Signed integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// Text literal.
    Str(String),
    /// Qualified name segments.
    Path(Vec<String>),
    /// List elements.
    List(Vec<SurfaceExpr>),
    /// Tuple elements.
    Tuple(Vec<SurfaceExpr>),
    /// Dictionary entries.
    Dict(Vec<(SurfaceExpr, SurfaceExpr)>),
    /// Callee followed by positional arguments.
    Call(Vec<SurfaceExpr>),
    /// One receiver and a member name.
    Attribute(Vec<SurfaceExpr>, String),
    /// Receiver and index.
    Index(Vec<SurfaceExpr>),
    /// Operator and two operands.
    Binary(String, Vec<SurfaceExpr>),
    /// Operator and one operand.
    Unary(String, Vec<SurfaceExpr>),
}

impl SurfaceExpr {
    /// Convert a validated public expression, preserving the finest supplied source range at every depth.
    fn into_ast(self, anchor: ast::Span) -> Result<ast::Spanned<ast::Expr>, String> {
        let span = inherited_span(self.span, anchor);
        let node = match self.kind {
            ExprKind::Scoped(_) => return Err("a scoped-surface desugarer must return ordinary Incan syntax".into()),
            ExprKind::Embedded(_) => return Err("an embedded-fragment hook must return ordinary Incan syntax".into()),
            ExprKind::NoneLiteral => ast::Expr::Literal(ast::Literal::None),
            ExprKind::Bool(value) => ast::Expr::Literal(ast::Literal::Bool(value)),
            ExprKind::Int(value) => ast::Expr::Literal(ast::Literal::Int(ast::IntLiteral::synthetic(value))),
            ExprKind::Float(value) => ast::Expr::Literal(ast::Literal::Float(ast::FloatLiteral {
                value,
                repr: value.to_string(),
                suffix: None,
            })),
            ExprKind::Str(value) => ast::Expr::Literal(ast::Literal::String(value)),
            ExprKind::Path(path) => path_expression(path, span)?,
            ExprKind::List(items) => ast::Expr::List(
                items
                    .into_iter()
                    .map(|item| Ok(ast::ListEntry::Element(item.into_ast(anchor)?)))
                    .collect::<Result<_, String>>()?,
            ),
            ExprKind::Tuple(items) => ast::Expr::Tuple(expr_nodes(items, anchor)?),
            ExprKind::Dict(items) => ast::Expr::Dict(
                items
                    .into_iter()
                    .map(|(key, value)| Ok(ast::DictEntry::Pair(key.into_ast(anchor)?, value.into_ast(anchor)?)))
                    .collect::<Result<_, String>>()?,
            ),
            ExprKind::Call(items) => call_expression(items, anchor)?,
            ExprKind::Attribute(items, name) => {
                let [base] = exact_nodes::<1>(items, anchor)?;
                ast::Expr::Field(Box::new(base), name)
            }
            ExprKind::Index(items) => {
                let [base, index] = exact_nodes::<2>(items, anchor)?;
                ast::Expr::Index(Box::new(base), Box::new(index))
            }
            ExprKind::Binary(op, items) => {
                let [left, right] = exact_nodes::<2>(items, anchor)?;
                ast::Expr::Binary(Box::new(left), binary_operator(&op)?, Box::new(right))
            }
            ExprKind::Unary(op, items) => {
                let [value] = exact_nodes::<1>(items, anchor)?;
                let op = match op.as_str() {
                    "-" => ast::UnaryOp::Neg,
                    "not" => ast::UnaryOp::Not,
                    "~" => ast::UnaryOp::Invert,
                    _ => return Err("malformed response: unknown unary operator".into()),
                };
                ast::Expr::Unary(op, Box::new(value))
            }
        };
        Ok(ast::Spanned::new(node, span))
    }
}

/// Use the input node's location only when an output node omits its own span.
fn inherited_span(span: Option<SourceSpan>, anchor: ast::Span) -> ast::Span {
    span.map(|span| ast::Span::new(span.start, span.end).with_expansion(span.expansion))
        .unwrap_or(anchor)
}

/// Map a public qualified name to the ordinary compiler access expression.
fn path_expression(path: Vec<String>, span: ast::Span) -> Result<ast::Expr, String> {
    let mut path = path.into_iter();
    let Some(first) = path.next() else {
        return Err("malformed response: empty expression path".into());
    };
    let mut value = ast::Expr::Ident(first);
    for (index, segment) in path.enumerate() {
        let child_span = span.expansion_child(&format!("path-prefix:{index}"));
        value = ast::Expr::Field(Box::new(ast::Spanned::new(value, child_span)), segment);
    }
    Ok(value)
}

/// Restore the ordinary call node, including method-call syntax selected by an attribute callee.
fn call_expression(items: Vec<SurfaceExpr>, anchor: ast::Span) -> Result<ast::Expr, String> {
    let mut items = items.into_iter();
    let Some(callee) = items.next() else {
        return Err("malformed response: call has no callee".into());
    };
    let callee = callee.into_ast(anchor)?;
    let args = items
        .map(|item| Ok(ast::CallArg::Positional(item.into_ast(anchor)?)))
        .collect::<Result<Vec<_>, String>>()?;
    match callee.node {
        ast::Expr::Field(base, method) => Ok(ast::Expr::MethodCall(base, method, Vec::new(), args)),
        _ => Ok(ast::Expr::Call(Box::new(callee), Vec::new(), args)),
    }
}

/// Decode the fixed operand count required by a public expression alternative.
fn exact_nodes<const N: usize>(
    items: Vec<SurfaceExpr>,
    anchor: ast::Span,
) -> Result<[ast::Spanned<ast::Expr>; N], String> {
    expr_nodes(items, anchor)?
        .try_into()
        .map_err(|_| "malformed response: wrong expression operand count".into())
}

/// Decode each expression while preserving its independent source range.
fn expr_nodes(items: Vec<SurfaceExpr>, anchor: ast::Span) -> Result<Vec<ast::Spanned<ast::Expr>>, String> {
    items.into_iter().map(|item| item.into_ast(anchor)).collect()
}

/// Resolve only the operators admitted by the published expression contract.
fn binary_operator(op: &str) -> Result<ast::BinaryOp, String> {
    use ast::BinaryOp as Op;
    Ok(match op {
        "+" => Op::Add,
        "-" => Op::Sub,
        "*" => Op::Mul,
        "/" => Op::Div,
        "//" => Op::FloorDiv,
        "%" => Op::Mod,
        "**" => Op::Pow,
        "@" => Op::MatMul,
        "|>" => Op::PipeForward,
        "<|" => Op::PipeBackward,
        "&" => Op::BitAnd,
        "|" => Op::BitOr,
        "^" => Op::BitXor,
        "<<" => Op::Shl,
        ">>" => Op::Shr,
        "==" => Op::Eq,
        "!=" => Op::NotEq,
        "<" => Op::Lt,
        ">" => Op::Gt,
        "<=" => Op::LtEq,
        ">=" => Op::GtEq,
        "and" => Op::And,
        "or" => Op::Or,
        "in" => Op::In,
        "not in" => Op::NotIn,
        "is" => Op::Is,
        "is not" => Op::IsNot,
        _ => return Err("malformed response: unknown binary operator".into()),
    })
}

/// Binding of the published spanned statement model.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceStatement {
    /// Source range inherited from the input when absent.
    span: Option<SourceSpan>,
    /// Published statement payload.
    kind: StatementKind,
}

/// Structured statement alternatives exposed by `std.vocab`.
#[derive(Deserialize)]
enum StatementKind {
    /// Empty statement.
    Pass,
    /// Expression statement.
    Expr(SurfaceExpr),
    /// Optional return value.
    Return(Option<SurfaceExpr>),
    /// Assignment to an existing binding.
    Assign(String, SurfaceExpr),
    /// New binding with explicit mutability.
    Let(String, bool, SurfaceExpr),
    /// Conditional branches.
    If(SurfaceExpr, Vec<SurfaceStatement>, Vec<SurfaceStatement>),
    /// Conditional loop.
    While(SurfaceExpr, Vec<SurfaceStatement>),
    /// Iteration with a named binding.
    For(String, SurfaceExpr, Vec<SurfaceStatement>),
    /// Explicit unsafe block.
    Unsafe(Vec<SurfaceStatement>),
}

/// Translate a statement sequence after source ranges have been validated for the entire response.
fn statements(values: Vec<SurfaceStatement>, anchor: ast::Span) -> Result<Vec<ast::Spanned<ast::Statement>>, String> {
    values
        .into_iter()
        .map(|value| {
            let span = inherited_span(value.span, anchor);
            let node = match value.kind {
                StatementKind::Pass => ast::Statement::Pass,
                StatementKind::Expr(expr) => ast::Statement::Expr(expr.into_ast(anchor)?),
                StatementKind::Return(expr) => {
                    ast::Statement::Return(expr.map(|expr| expr.into_ast(anchor)).transpose()?)
                }
                StatementKind::Assign(name, expr) => assignment(name, ast::BindingKind::Reassign, expr, span, anchor)?,
                StatementKind::Let(name, mutable, expr) => {
                    let binding = if mutable {
                        ast::BindingKind::Mutable
                    } else {
                        ast::BindingKind::Let
                    };
                    assignment(name, binding, expr, span, anchor)?
                }
                StatementKind::If(condition, then_body, else_body) => ast::Statement::If(ast::IfStmt {
                    condition: ast::Condition::Expr(condition.into_ast(anchor)?),
                    then_body: statements(then_body, anchor)?,
                    elif_branches: Vec::new(),
                    else_body: Some(statements(else_body, anchor)?),
                }),
                StatementKind::While(condition, body) => ast::Statement::While(ast::WhileStmt {
                    condition: ast::Condition::Expr(condition.into_ast(anchor)?),
                    body: statements(body, anchor)?,
                }),
                StatementKind::For(name, iter, body) => ast::Statement::For(ast::ForStmt {
                    pattern: ast::Spanned::new(ast::Pattern::Binding(name), span.expansion_child("for-binding")),
                    iter: iter.into_ast(anchor)?,
                    body: statements(body, anchor)?,
                }),
                StatementKind::Unsafe(body) => ast::Statement::Unsafe(ast::UnsafeStmt {
                    body: statements(body, anchor)?,
                }),
            };
            Ok(ast::Spanned::new(node, span))
        })
        .collect()
}

/// Construct an ordinary binding while retaining the provider's declaration and value spans.
fn assignment(
    name: String,
    binding: ast::BindingKind,
    value: SurfaceExpr,
    span: ast::Span,
    anchor: ast::Span,
) -> Result<ast::Statement, String> {
    Ok(ast::Statement::Assignment(ast::AssignmentStmt {
        binding,
        name,
        name_span: span,
        ty: None,
        value: value.into_ast(anchor)?,
    }))
}
